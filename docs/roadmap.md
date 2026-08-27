# 開発ロードマップ

- [x] **CW（閲覧注意）機能（#229）** — 投稿フォームにCWトグルを追加し、ONにするとCWガイド文
  入力欄（100書記素まで）が現れる。バックエンドは`CreateNoteRequest.content_warning`
  （Misskey本家`cw`パラメータもエイリアス）を受理し`posts.content_warning`（Fedi受信CW用の
  既存カラムと同じ）を構築、AP配送では`Create(Note)`の`summary`フィールドとしてそのまま
  送信する（本文・添付・アンケート・引用は通常通り配送、Fedi側クライアントが`summary`の
  有無でCW UIを出し分ける）。BskyにはCW/隠しコンテンツの概念が無いため、CWが設定された
  投稿はBsky embed選択（#227/#228）の候補選択・引用embedを一切行わず、常に「投稿詳細ページ
  URL＋`#open_cw`ハッシュ」1件だけを、`title`固定文字列`"Open"`・description/thumb無しの
  リンクカードとして添付する（本文＝投稿本文ではなくCWガイド文に差し替える）。そのため
  フロントエンドはCWが有効な間、Bsky embed選択のラジオボタンリスト自体を表示しない
  （画像/GIF/動画/URL/アンケートの添付・作成自体は妨げない）。DM（`visibility=="direct"`）
  ではCW作成を禁止する。詳細: `docs/protocols.md` 3節「CW」、`docs/ui_spec.md` 2.4b節、
  `docs/database.md`

- [x] **アンケート機能（#228）** — Fediverse仕様のアンケート（選択肢2〜10件・単一/複数選択・期限なし/日時指定/経過時間）を投稿フォームに追加した。バックエンドは`CreateNoteRequest.poll`（選択肢・複数選択可否・期限）を受理し`posts.poll`（Fedi受信Question用の既存カラムと同じ形）を構築、AP配送では`build_create_note_activity`が`Question`型（`oneOf`/`anyOf`・`endTime`）として送信する（リモートからの投票受信は既存の`handle_poll_vote`がローカル/リモート問わず汎用的に処理するため追加実装不要）。Bskyには投票概念が無いため、アンケート付きポストはBsky embed選択（#227）の新候補`Poll`（常に最優先）として、投稿自身の詳細ページURLを選択肢名だけのプレーンテキスト箇条書き（HTMLタグ・得票バー無し、投稿の言語が決定できないため見出し文も無し）をdescriptionにしたリンクカードで添付する。投票UI・集計は既存の`poll_votes`・投票API・`NoteCard`表示がそのまま動く。`NoteCard`のアンケート表示には「結果を見る」ボタンの右隣に期限までの残り時間（1分未満は秒単位、1分以上1時間未満は分単位、1時間以上1日未満は「時間+分」、1日以上は「日+時間+分」、1秒ごとにカウントダウン）も追加し、購読者が1人以上いる間だけ`setInterval`を1本に集約する共有タイマーストア（`stores/secondTicker`、`useSyncExternalStore`）に`PollCountdown`コンポーネント単体が購読することで実現した（`NoteCard`全体は再描画されない）。詳細: `docs/protocols.md` 3節「アンケート」、`docs/ui_spec.md` 2.4b節・アンケート節、`docs/database.md`

- [x] **ローカルポストへの添付物の体系整理（#227）** — 投稿フォームで複数ファイル添付（最大10件、画像/アニメGIF/動画/音声混在可）に対応し、Bluesky配送がONかつ添付候補（静止画グループ・アニメGIF・動画/音声・本文中URL）が2種類以上ある場合に、どれをBsky embedにするか選ぶラジオボタンリストを本文欄の下に表示する。バックエンドは`CreateNoteRequest.bsky_embed_choice`（`Images`/`Attachment{id}`/`Url{url}`）で選択を受け取り、`resolve_bsky_embed`（`crates/seiran-api/src/handlers/notes/delivery.rs`）が省略時は固定優先順位（静止画→アニメGIF→動画/音声→本文URL、いずれも先頭優先）で自動選択する。URL選択時は選択URLのOGPを同期取得してBsky embedを組み立てると同時に`post_link_cards`へ保存し、seiranローカルの表示にも同じURLカードを反映する。動画/音声添付を選択（または自動選択）した場合のBskyパイプライン結合待ちは、対象1件のみを追う`Job::BskyPostCommitDeferred`に簡素化。あわせて`media_files.is_animated_image`を新設し、ローカルアップロードの静止画とアニメGIFを判別できるようにした。ラジオボタンリストが表示されない場合（Bluesky配送オフ、またはCW有効中）でも本文中にURLがあれば、代わりにチェックボックスリスト（`CreateNoteRequest.link_card_urls`、複数選択可）でURLリンクカードを添付できる。チェックボックスリストからラジオボタンリストへ表示が切り替わった瞬間、チェック済みURLのうち最もインデックスの小さいものがラジオボタンリストの選択へ引き継がれる。詳細: `docs/protocols.md` 3節「Bsky embed選択」、`docs/ui_spec.md` 2.4b節、`docs/database.md`
- [x] **ATP標準クライアントからの投稿作成・削除（`com.atproto.repo.createRecord`/`deleteRecord`等でのapp.bsky.feed.post対応）** — bsky.app等のATP標準クライアントから直接投稿・削除できるようにした（従来は専用エラーで一律拒否していた）。`createRecord`/`putRecord`/`applyWrites`が`app.bsky.feed.post`を受けた場合、`post_from_record::create_post_from_record`がレコード（text/facets/embed/reply）を`posts`テーブルへ変換し、Jetstream受信と共通化した`facets`/`embed`解析ロジックでリンク・メンション・画像・動画・引用・リプライを復元、ハッシュタグ抽出・通知・Fedi配送（`ApDeliveryKind::PostToFollowers`）まで行った上でクライアント提供のレコードをそのままATPリポジトリへコミットする。`deleteRecord`/`applyWrites#delete`は`post_from_record::delete_post_by_rkey`が`handlers::notes::delete_note`と同じ論理削除・Fedi Delete配送・ATPリポジトリ削除を行う。あわせて`uploadBlob`を拡張し、Bsky公式動画パイプラインのコールバック専用（サービス間認証JWT）だった認証を、通常のユーザーセッションJWTでも受け付けるようにした（従来は標準クライアントが画像・動画を添付しようとすると`uploadBlob`自体が失敗していた）。詳細: `docs/protocols.md` 8節
- [x] **フォロー中Bskyユーザーによるリモート投稿へのリポストをタイムラインへ反映** — Jetstreamの`app.bsky.feed.repost`受信時、AP `Announce`受信と対称にリポストをタイムライン投稿として`posts`へ保存するようになった（従来はローカル投稿宛の通知作成のみで、フォロー中Bskyユーザーが他のリモート投稿をリポストしてもタイムラインに現れなかった）。リポスト対象が未取り込みなら`app.bsky.feed.getPosts`でAppViewから直接フェッチする。あわせて、この直接フェッチ経路（`fetch_single_bsky_post`/`upsert_bsky_post`、検索結果保存・ピン留め投稿同期・「開く」機能とも共用）でも`record.embed`から画像・動画・GIF・URLカードを復元するようにした（従来は本文のみでBsky側の添付が欠落していた）。embed解析ロジックはJetstream通常投稿取り込みと共通化し`seiran-common::atp::embed`に集約。詳細: `docs/protocols.md` 8節
- [x] **NoteCardリモートサーバー表示・長いニックネームのはみ出し修正** — NoteCardヘッダーを表示名+日付／アカウントID+リモートサーバー表示の2行構成へ再編し、長いニックネームがカード右端・投稿日付にはみ出す不具合（`.userContainer`の`min-width:0`欠落が原因）を修正。あわせて`body`（`html`側は触らない）に`overflow-x: hidden`を設定し、はみ出しがモバイルのフローティングボタン位置をずらす連鎖を防止。Fedi/Bskyのリモート投稿には、アカウントID行の右にサーバーアイコン＋サーバー名称（Bskyは固定表示、Fediは`remote_instance_meta`のnodeinfoキャッシュ由来）を背景色付きで表示する新UIを追加し、ローカル投稿の配送先バッジもBsky分は🦋絵文字からBlueskyロゴSVGに変更。バックエンドはMisskey API `UserLite.instance`準拠の`instance`フィールド（`themeColor`/`iconUrl`含む）をnotes API・Misskey互換APIの両方に追加し、リモートインスタンスのnodeinfo・サーバーアイコン（`<link rel="icon">`/`/favicon.ico`）を`RemoteInstanceInfoResolve`ジョブでベストエフォート取得・キャッシュする。既存の全リモートドメインを起動時に一括バックフィルし、新規デプロイ直後の大量未解決状態を素早く解消する。詳細: `docs/database.md`、`docs/architecture.md`、`docs/protocols.md`、`docs/ui_spec.md`

- [x] **Unicode絵文字のtwemoji統一表示** OS/ブラウザごとにグリフが異なるUnicode絵文字（本文・表示名・リアクション・絵文字ピッカー・装飾アイコン等）を、jdecked/twemojiのSVGをセルフホストして統一表示。詳細: `docs/ui_spec.md`「Unicode絵文字の表示（twemoji）」節

- [x] **iPhoneのフォーム自動ズーム防止（#208）** `ComposerEditor`をはじめ、絵文字ピッカー検索欄・認証フォーム・DM・設定画面・管理画面など全ての`input`/`textarea`/`select`の実入力文字サイズを16pxに固定し、iOS Safariのフォーカス時自動拡大を防止。

- [x] **Blueskyリポスト・引用通知（#206）** Jetstreamで `app.bsky.feed.repost` を購読し、ローカル投稿へのリポストを通知する。取り込み対象のBsky投稿がローカル投稿を引用した場合も引用通知を生成する。詳細: `docs/protocols.md` 8節

- [x] **フォロー承認待ちの解除（#204）** プロフィールの承認待ち表示横とNoteCardのフォロー状態スイッチから、承認前のフォローリクエストを解除できる。

- [x] **スマホ下部フローティングナビへ「ホーム」ボタン追加（#180）** — メニュー・検索・通知・ホーム・投稿の順（ホームは投稿の左隣）で並ぶよう、モバイル幅（`max-width: 768px`）でのフローティングボタン群に🏠ボタンを追加し、タップで`/`へ遷移する。
- [x] **スマホ下部フローティングナビの狭幅崩れ修正** — 5個のボタンをそれぞれ独立した`position: fixed`（px絶対値＋%相対値混在）で配置していたため、画面幅が狭いと隣接ボタン同士が重なり、投稿ボタンが視覚的に隠れる不具合があった。5個を1つの`position: fixed`コンテナにまとめ、flexboxの`justify-content: space-between`で均等配置する方式に変更し、画面幅にかかわらず重なり・はみ出しが起きないようにした。
- [x] **絵文字管理者ロール `emoji-editor` を追加（#179）** — `user_role` ENUM に `emoji-editor` を追加（権限の強さ: admin > moderator > emoji-editor > user）。絵文字管理権限を `moderator` にも付与し、管理画面のトピック別アクセス制御（`frontend/src/lib/roles.ts` の `getAdminTopics`）を導入して、権限のないトピックはタブごと非表示にする。`moderator` は調停者として「通報」（凍結・投稿削除・連合転送を含む）と「絵文字」タブに、`emoji-editor` は「絵文字」タブのみアクセス可能。バックエンドは `require_admin`（admin専用）・`require_emoji_admin`（admin/moderator/emoji-editor、絵文字系管理API全8箇所に適用）・`require_report_moderator`（admin/moderator、通報系管理APIに適用）を分離。詳細: `docs/database.md`
- [x] **プロフィールBioのカスタム絵文字展開（#169）** — ノート本文・表示名と同様、プロフィールの自己紹介文（bio）中の`:shortcode:`を画像に展開する。ローカルアクターは`custom_emojis`照合、リモートFediアクターはAP `tag`由来の`actors.emoji_map`を使用し、Bskyアクターは展開しない。詳細: `docs/ui_spec.md` 2.2節
- [x] **URL・ユーザーIDから「開く」（#165）** — bsky.app/ActivityPub URL、AT URI、`@`ユーザーID、`did:plc:` DIDを解決・必要時取り込みして詳細画面へ遷移する。QR連続認識と2秒間隔のOCRも提供する。

- [x] **Ariaのハッシュタグ投稿一覧API互換（#158）** `POST /api/notes/search-by-tag`を追加し、既存の専用ハッシュタイムラインをMisskey形式で返す。
- [x] **投稿検索へのBluesky AppView統合（#146）** AppViewの未知actor/postをDBへ保存してローカル検索結果とブレンドし、`until_id`の時刻指定とDB-onlyの`since_id`ページングに対応する。稼働AppViewホストへの接続、検索結果アバター、Misskey互換POST検索、モバイル検索ボタンも含む。
- [x] **Ariaのカスタム絵文字API検出互換（#145）** `POST /api/endpoints`で`emojis`を含む実装済みMisskey API一覧を返し、既存の`POST /api/emojis`へ誘導する。
- [x] **リレー経由Fedi投稿の本文カスタム絵文字補完（#148）** Create/Announce双方の埋め込みNoteで未知のEmoji tagが欠落していても、本文に未解決shortcodeがある場合だけcanonical Noteを取得し、`emoji_map`とリモート絵文字カタログを補完する。Announce経由で未登録の元ポストをフェッチする経路（`fetch_and_save_note`）も、絵文字・可視性・引用・リプライ・CW/投票・ハッシュタグ・添付ファイルをCreate受信と同じロジックで処理する。

進捗管理用のチェックリスト。完了済みフェーズは概要のみ、未完了項目は詳細に残す。
機能追加を完了したら該当箇所に `[x]` を入れ、コードの変更と同じコミットに含めること（`/home/yuba/seiran/CLAUDE.md` 参照）。

- [x] **タイムライン可視性のおさらい（#105）** — HTL/STLでは自分・フォロー中のひかえめ/プライベート投稿を表示し、LTL/GTLでは投稿者本人のものも含めて両方を完全に除外。バックエンドSQL、フロントエンド最終防御、ユニット/E2Eで固定。

## 完了済みフェーズ（概要）

- [x] **CWガイド文のカスタム絵文字展開（#201）** — 通常投稿カード・引用カードのCW警告文中にある`:shortcode:`を、投稿の絵文字マップで画像へ展開する。
- [x] **MitraのURI形式Follow承認に対応（#200）** — `Accept.object` が埋め込みオブジェクトではなくFollow ActivityのURI文字列でも、送信元・送信先actor ID入りのActivity IDから対象関係を復元し、承認actorを検証してフォロー待機状態を解消する。

- [x] **統一通報機能（#107）** — ローカル・Fedi・Bskyの投稿/ユーザー通報、管理台帳・内部コメント・対処、ActivityPub Flag / Bluesky Moderation Service転送。
- [x] **国際化の言語追加（#138）** — 日本語・英語に加えて中国語・韓国語・スペイン語・ドイツ語・フランス語へ対応。フロントエンドの日本語ハードコードを翻訳キーへ移し、全言語のキーと補間変数の一致を自動検証する。
- [x] **リモートFedi actor・投稿の重複修復（#139）** — 破損したUNIQUE indexが同じAP URIのactor・投稿を複数行へ分裂させ、プロフィール/HTLから新着が欠落していたデータを、全外部キーと複合UNIQUEの意味を保ってcanonical IDへ統合し、indexを再構築するマイグレーションを追加。

- [x] **フェーズ1: DBスキーマ ＆ 統一ID採番** — `posts`/`actors`/`follows` 等の統一エンティティ設計、Snowflake ID採番エンジン。詳細: `docs/database.md`
- [x] **フェーズ2: ローカル認証 ＆ MiAuth互換** — Argon2+JWT、MiAuth、メール確認・パスワードリセット、`secrets.toml` 自動生成。詳細: `docs/architecture.md` 4節
- [x] **フェーズ3: ジョブキュー ＆ 統合バイナリ化** — `JobQueue` trait、InMemory/Redis切替、`seiran-server` の `--role` 分割。詳細: `docs/architecture.md` 3・5節
- [x] **フェーズ4: マルチプロトコル通信エンジン** — AP/ATP双方向フェデレーション、クロスプロトコル配送（リポスト・引用・リプライ）、リアクション相互配送。詳細: `docs/protocols.md`
- [x] **フェーズ4.5: フロントエンドMVP** — React+Vite+TypeScript初期版、ローカル/ホームタイムライン、投稿・フォロー・プロフィールの基本画面
- [x] **フェーズ4.6: メディア・管理機能** — S3互換オブジェクトストレージ統合、画像/動画/音声アップロード、管理画面（ユーザー・絵文字・ストレージ設定）
- [x] **フェーズ5: 重複排除・マージエンジン** — ループバック/他seiran間/一般ブリッジの3シナリオ対応。詳細: `docs/protocols.md` 5節
- [x] **フェーズ6: 検索セッション管理** — `SearchSession`、ブレンドアルゴリズム（InMemory実装のみ）。詳細: `docs/architecture.md` 6節
- [x] **フェーズ7: 3ペインUI ＆ Misskey API互換** — 3ペインレイアウト、リアクション・通知・ピン留め・リスト機能、Misskey互換エンドポイント一式。詳細: `docs/ui_spec.md`, `docs/protocols.md` 6節
- [x] **フェーズ7.5: フロントエンド国際化 ＆ エラーメッセージ改善** — react-i18next導入（英語/日本語、ブラウザ言語設定への自動追従）、バックエンドエラーコード全種の日英メッセージ化、トースト通知、401時の自動ログアウト＋ログイン画面誘導。詳細: `docs/architecture.md` 8節
- [x] **フェーズ7.6: 本文中のリンク・メンションのクリック可能化** — Bsky facet（`#link`/`#mention`）・AP `<a href>` を内部リンクマーカー`[text](url)`としてMisskey API互換の`text`に埋め込み、フロント`RichText`コンポーネントでMarkdownリンク・生URL・`@mention`をクリック可能な要素へ変換。Bskyメンションはハンドル可変性に対応するため表示時に都度DID解決する。送信側（seiranユーザー投稿→Fedi/Bsky配送）もローカル/Bskyハンドル/Fediverse形式すべてのメンションでfacet・AP `tag[]`+アンカーを付与し、Bsky配信時は変換後テキストの文字数上限（300書記素/3000バイト）を投稿受理前に同期検証する。詳細: `docs/protocols.md` 6節
- [x] **フェーズ7.7: 投稿詳細・プロフィールページのOGP対応** — SPAの素のindex.htmlには`<meta>`が無いため、`/notes/:id`・`/@:handle`（AP Accept除く）は常にバックエンド（`seiran-api`）がSPAのindex.htmlを取得してOGP `<meta>` + Twitter Cardを注入して返す（bot判定は行わず未知のクローラーにも対応、投稿/アクター未発見時は`<meta>`無しでSPAをそのまま返す）。詳細: `docs/architecture.md` 8.1節
- [x] **フェーズ7.8: ハッシュタグ機能** — `hashtags`/`post_hashtags`/`pinned_hashtags` によるポスト⇔タグのm:n永続化。ローカル投稿・AP受信・Bsky受信いずれも最終的な `posts.body` から共通のスキャン（`seiran_common::hashtag::extract_hashtags`）で抽出するため、出自を問わず同じハッシュタイムライン（`/tags/:name`）に合流する。ハッシュタイムライン画面から「ホーム画面に追加」（`pinned_hashtags`、ホームのフィードタブ化）・「このハッシュタグでポスト」（`ComposerContext.openCompose` によるプリフィル投稿ダイアログ）。送信側（ローカル投稿→Bsky/AP配送）も `app.bsky.richtext.facet#tag`・AP `{"type":"Hashtag"}` タグ（自インスタンスの `/tags/:name` へのアンカー）を付与し、他クライアント上でも本物のハッシュタグとして認識される。受信側はMastodon等がハッシュタグアンカーにも`class="mention hashtag"`を付与する（メンションと`mention`トークンを共有する）ケースを`rel="tag"`で判別し誤ってメンション扱いしないようにする回帰修正込み。詳細: `docs/database.md`、`docs/protocols.md` 6節
- [x] **フェーズ7.9: ダイレクトメッセージ機能** — `visibility='direct'`投稿を`posts`にそのまま格納し宛先（`post_recipients`）・スレッド起点伝播コピー（`thread_root_post_id`）・既読状態（`dm_read_states`）で管理。Fedi宛先は宛先個人のみへのAP配送、Bsky宛先は`chat.bsky.convo`（自己署名サービス認証JWT、送信は`Job::BskyDmSend`、受信は`seiran-atp-repo::bsky_dm_poll`の定期ポーリング）。Bsky宛先は1対1のみ・文字数上限1000書記素・メディア添付不可。フロントエンドは`MessagesPage`（右ペイン=セッション一覧、中央ペイン=時刻順メッセージ履歴+送信フォーム）、`RecipientPicker`（宛先chip入力）、左ペイン未読バッジ。詳細: `docs/database.md`、`docs/protocols.md` 9節、`docs/ui_spec.md` 2.5節
- [x] **フェーズ7.10: ブロック・ミュート機能** — プロフィール画面に対ユーザー操作メニュー（`ActionsMenu`、フォロー中/フォロー・ミュート・ブロックを統合、フォローは独立ボタンも併設）を新設。ブロックはBsky準拠の定義（フォロー関係強制解除＋相互完全非表示）を採用し、相手がBskyなら`app.bsky.graph.block`コミット、相手がFediならAP `Block`配送。タイムライン・通知の相互非表示・プロフィール本文/key-valueの非表示はSQL関数`actor_is_hidden_for_viewer`と`is_blocked_by`判定に集約、フォロー・リプライ・リアクション・引用投稿・リポスト・DM送信の書き込みもAPIレベルで拒否する。相手発ブロック（Fedi/Bskyリモートユーザーが自分をブロックした場合）も検知して同じ制限を対称に働かせる：Fedi側はAP `Block`受信時に記録、Bsky側は`app.bsky.graph.block`の無絞り込みJetstream監視（`seiran-atp-repo::bsky_block_watch`）でリアルタイム検知する。ミュートはFedi/Bsky共通のローカル効果のみ（AP/ATP配送なし）。詳細: `docs/database.md`、`docs/protocols.md` 10節、`docs/ui_spec.md` 2.2節
- [x] **フェーズ7.11: メンション通知** — 本文中で`@username`形式によりローカルユーザーが言及された場合に通知（`type="mention"`）を作る。投稿の出自（ローカル投稿・Fedi受信・Bsky受信）ごとに解決経路を持ち、いずれも自己メンションは通知しない。通知一覧・クイック通知パネルへのリアルタイム反映は既存のフォロー/リアクション通知と同じ仕組み（`NotificationRepository`・`StreamHub`）に統合。詳細: `docs/protocols.md` 8節
- [x] **フェーズ7.12: リプライ通知 ＆ 通知パネルのリンク化** — 自分の投稿に返信が付いた場合に通知（`type="reply"`）を作る。投稿の出自（ローカル投稿・Fedi受信・Bsky受信）ごとに解決経路を持ち、リプライ先がローカルユーザーの投稿の場合のみ通知、自己リプライは通知しない。あわせてクイック通知パネル（`NotificationsPanel`）を全面的にリンク化: 通知者のユーザー名は種別によらず常にプロフィールページへのリンク、リプライ・リアクション・メンション通知は通知文全体が対象ポストへのリンクになる。詳細: `docs/protocols.md` 8節、`docs/ui_spec.md` 2.1節
- [x] **フェーズ7.13: カスタム絵文字リアクション** — ローカルユーザーがカスタム絵文字（`:shortcode:`）でリアクションできるようにした。バックエンドは`validate_reaction_content`をUnicode/カスタムの判別のみ行う純関数に整理し、`create_reaction`が`EmojiRepository::find_url_by_shortcode`でURL解決・実在確認する。AP配送はMisskey/Fedibird互換の`EmojiReact`＋`tag: [{type: Emoji, ...}]`まで対応（受信側の`build_emoji_map`と対称）、ATPはLike＋`emoji`拡張フィールドのベストエフォートのまま。フロントは`ReactionPicker`を刷新し、`Modal`内の`EmojiPickerPanel`（検索欄＋よく使う/絵文字/カスタムのタブ＋グリッド）に統合。Unicode絵文字データセット（`unicode-emoji-json`）は`React.lazy`で遅延ロード。「よく使う」は自分の現在のリアクションを`GET /api/reactions/frequent`で頻度集計した近似値。あわせて`POST /api/admin/emojis`の500エラー（`media_file_id`をJS `Number()`変換すると53bit精度の壁でsnowflake IDが破損し外部キー違反になっていた）を、リクエストボディを文字列で受けてサーバー側でparseする方式に修正。管理画面の絵文字一覧に画像プレビューも追加。絵文字ZIPインポート（`/api/admin/emojis/import`）はボディサイズ上限未設定で大きいZIPが`multipart/form-data`解析エラーになる不具合、およびアニメーションGIF/WebP/APNGが`process_image`で静止画WebPへ変換されてしまう不具合（`image`クレート0.25はアニメーション書き出し未対応のため、アニメーション画像はリサイズ・再エンコードせず元バイト列のまま保存する方式に修正）も解消。加えて`ReactionChips`の各チップにホバーすると、そのリアクションを付けたアクター一覧（アイコン＋名前）をポップオーバー表示する機能（`GET /api/notes/:id/reactions/:content/actors`）を追加。kmyblue等が絵文字リアクション対応を検出できるよう、`GET /nodeinfo/2.1`の`metadata.features`に`"emoji_reaction"`を追加（#167）。詳細: `docs/protocols.md`、`docs/database.md`、`docs/ui_spec.md` 2.2b節
- [x] **フェーズ7.13a: 通知対象ポストのホバープレビュー** — リアクション・メンション・返信通知へのマウスオーバーで対象ポストの投稿者と本文を表示する。返信先ポップアップを再利用可能な`NoteHoverPreview`へ共通化し、初回ホバー時のみ取得・120ms遅延クローズの操作感を統一。詳細: `docs/ui_spec.md` 2.1節
- [x] **フェーズ7.14: 「リモートで表示」バナー** — ポスト詳細・プロフィール画面に、対象がローカルアクターでない場合の共通バナー（`RemoteBanner`）を追加。Fedi由来はAP URI（末尾`/activity`はMisskey・Mastodon等のActivity id慣習のため除去）、Bsky由来は`https://bsky.app/profile/{did}[/post/{rkey}]`へ別タブで遷移するリンクを表示する。`NoteResponse`に`remoteUrl`（`posts.ap_object_id`/`at_uri`から算出）を追加。リポストラッパーはリポストした人自身（`note`）でリモート判定・リンク先を決める。詳細: `docs/ui_spec.md` 3.3節
- [x] **フェーズ7.15: プロフィール画面のフォロー中/フォロワー一覧（#56）** — プロフィール右ペインをタブシート化（【投稿】【フォロー中】【フォロワー】、`Tabs`コンポーネント）し、中央ペインのフォロー数・フォロワー数バッジをクリックすると対応タブへ切り替わる。バックエンドは`FollowRepository::list_following`/`list_followers`（`follows.id`によるカーソルページネーション、`actor_is_hidden_for_viewer`でブロック関係を除外）と`count_relations`を追加、`GET /api/users/following`・`/api/users/followers`・`ProfileResponse.following_count`/`follower_count`として公開。DB未登録のリモートアクター（`actor_id`を持たない）はフォロー一覧タブ自体を出さず従来通り投稿一覧のみ表示する。詳細: `docs/ui_spec.md` 2.2節
- [x] **フェーズ7.16: 設定画面（#55）** — メインメニューに「設定」を新設し、`/settings`（メニュー）・`/settings/account`（アカウント設定）・`/settings/mutes-blocks`（ミュート・ブロック管理）・`/settings/appearance`（表示設定）を追加。アカウント設定はメール/DID表示、現在のパスワード確認付きパスワード変更（`POST /api/account/change-password`、`LocalAuthProvider::verify_password`/`hash_password`を再利用）、退会（旧プロフィール編集画面から移動）を集約する。ミュート・ブロック管理は`MuteRepository::list_muted`/`BlockRepository::list_blocked`（新規追加、最大200件・カーソルページネーションなし）による対象者一覧＋解除ボタンをタブ切り替えで表示する。表示設定は言語（自動/日本語/英語）を`POST /api/account/language`で`users.language_preference`に保存し、`i18n.changeLanguage()`で即時反映する。アプリトークン（発行済みトークンの一覧・無効化、およびMiAuth連携を介さない画面からの直接発行）は`app_tokens`テーブル（#60、詳細: `docs/database.md`）を新設して実装済み。`/settings/app-tokens`で一覧表示・直接発行・個別無効化ができる。詳細: `docs/ui_spec.md` 2.7節
- [x] **メールアドレス変更（#59）** — アカウント設定（`/settings/account`）に新アドレス入力フォームを追加。`email_changes`テーブル（`password_resets`と同型のワンタイムトークン方式、`user_id`紐付き）に変更リクエストを保存し新アドレス宛に確認メールを送信、`POST /api/account/email/confirm-change`でリンク踏み時点のトークン消費と`users.email`更新を行う（`/verify-email-change?token=...`がフロントの着地先）。既存の新規登録用`email_verifications`はuser_idを持たないため使い回さず専用テーブルとした。詳細: `docs/database.md`、`docs/ui_spec.md` 2.7節
- [x] **TOTP二段階認証（#65 前半）** — 認証アプリ設定、10件の使い切りリカバリーコード、ログイン時の二段階検証、登録メール経由の解除を実装。シークレットは暗号化、リカバリーコードはArgon2ハッシュのみを保存する。詳細: `docs/architecture.md` 4節、`docs/database.md`、`docs/ui_spec.md` 2.7節
- [x] **複数パスキー（#65 後半）** — WebAuthnによるパスワードレスログイン、複数credentialの登録・名前付き一覧・削除に対応。チャレンジは5分で失効し、一度だけ消費する。ログインはメールアドレス/ユーザー名の入力不要なusernameless（discoverable credential）方式。
- [x] **管理画面の二段階認証状況（#65）** — ユーザーごとのTOTP状態・パスキー登録数表示と、管理者によるTOTP強制解除に対応。
- [x] **Fediverseリレー参加（#140）** — 複数リレー管理、専用actorのFollow/UndoとAccept/Reject状態管理、公開投稿のみの配送、管理画面UIに対応。
- [x] **添付画像のライトボックス表示（#64）** — `NoteCard`の添付画像クリックを新規タブ遷移からページ内ライトボックス（`ImageLightbox`）表示に変更。バックエンドの変更なし（フロントエンドのみ）。詳細: `docs/ui_spec.md` 2.2b節
- [x] **添付画像Lightboxのページング（#153）** — 複数画像を左右矢印キー・左右スワイプ・前後ボタンで移動し、端では移動不能なボタンを非表示にする。詳細: `docs/ui_spec.md` 2.2b節
- [x] **Fedi投稿のCW・アンケート・画像NSFW（#102）** — ActivityPub受信時にCW、アンケート集計、画像単位の閲覧注意を保存し、NoteCard/LightBoxを安全側の解除状態遷移で表示。
- [x] **リモートメディアプロキシ（#87）** — Misskey互換 `GET /proxy?url=...` をSSRF対策・リダイレクト再検証・サイズ/時間/Content-Type制限付きで追加。別オリジンの添付、アバター、本文・リアクション絵文字を中継し、管理画面から外部Misskey互換プロキシへ切替可能。詳細: `docs/architecture.md`、`docs/ui_spec.md`
- [x] **未設定アバターの backend 生成（#211）** — actor ID から決定論的な顔 SVG を生成・配信し、Misskey互換 API と ActivityPub Actor/Update のアバターへ反映。詳細: `docs/architecture.md`、`docs/protocols.md`
- [x] **管理画面タブシートの画面上部張り付き・左右スワイプ（#66）** — `/admin` のタブシート（`Tabs`）に、プロフィール画面のフィードタブと同じsticky手法をオプトインで適用（`sticky`/`top` props、直上の見出しの実高さぶんオフセット）し、下スクロール時に画面上部へ張り付くようにした。あわせてコンテンツ領域に既存の`useSwipe`フックを適用し、モバイルでの左右スワイプによるタブ切り替えに対応。バックエンドの変更なし。詳細: `docs/ui_spec.md` 2.8節
- [x] **リモートFediユーザーのフォロー中/フォロワー全件取得・表示（#68）** — プロフィール画面で、`follows`テーブル（seiranが認知している関係のみ）とは独立に、相手のAPアクタードキュメントの`following`/`followers`OrderedCollectionへ直接問い合わせて全件取得する。短タイムアウト（200ms）の同期取得を試み、失敗/タイムアウト時は`Job::RemoteFollowListSync`をバックグラウンドで積み`remote_follow_snapshots`テーブルへキャッシュ、次回リロードで反映される。未登録アクターは`Job::RemoteActorResolve`でプロフィールを解決する。フロントは`ProfilePage`でプロフィール取得直後にタブが開かれる前から先読みを開始し（`remoteFollowSummaryCache`）、「フォロー中/フォロワー」タブにローカルDB未把握の項目を見出しで分けず同じ見た目の1つのリストとして混ぜて表示（既知アクターはアバター等付き、未知はハンドル文字列のみ）。プロフィールカードのフォロー中/フォロワー人数もローカル・リモートをブレンドした実数（`total_count`）を表示する。詳細: `docs/protocols.md` 2節、`docs/database.md`
- [x] **カスタム絵文字のライセンス情報（#63）** — `custom_emojis.license`カラム（Misskey ZIPインポート #50 で追加済みだったが、インポート時にしか設定できなかった）を、手動での絵文字作成・編集からも設定できるようにした。`POST /api/admin/emojis`・`PATCH /api/admin/emojis/:id`のリクエストボディに`license`（1行テキスト・任意項目、改行を含む場合は`INVALID_LICENSE`で拒否）を追加。管理画面の絵文字追加フォームとインライン編集（タグ編集と統合）にライセンス入力欄を追加。zipインポートは既存実装のまま。
- [x] **投稿本文のメンション・カスタム絵文字入力支援（#94）** — `PostComposer`の本文欄をプレーンテキストへ直列化可能な`ComposerEditor`へ置換し、`@`/`:`候補、キーボード選択、ローカル・完全修飾Fedi/Bsky IDの既知/未知色分け、入力中のcaret維持、NoteCardと同じ右境界ルールによるカスタム絵文字の不可分画像表示と境界Backspace操作に対応。詳細: `docs/ui_spec.md` 2.1節
- [x] **投稿フォームの改行・IMEプレースホルダー修正（#124, #125）** — `ComposerEditor`のEnter改行を値とcaretの同期処理に統一し、1回のEnterで改行が1個だけ入るよう修正。プレースホルダーはcontenteditableの空状態に連動させ、IME未確定文字の入力開始時点で隠れるようにした。
- [x] **Misskey互換API フォロイー/フォロワー一覧・リアクションユーザー一覧（#81）** — Ariaがプロフィール画面のフォロー数/フォロワー数バッジ、およびリアクション長押しから叩く`POST /api/users/following`・`/api/users/followers`・`POST /api/notes/reactions`が未実装で405 Method Not Allowedになっていた不具合を修正。カスタムAPIの同パス`GET`と共存させる形で`POST`ハンドラを追加し、既存の`FollowRepository::list_following`/`list_followers`・`ReactionRepository::actors_for_reaction`をMisskeyワイヤー形状（`MisskeyFollowRelation`・`MisskeyNoteReaction`）に変換して返す。詳細: `docs/protocols.md` 7節
- [x] **ソーシャルタイムライン・グローバルタイムライン（#78）** — ホーム画面のフィードタブに「ソーシャル」（自分+フォロー中+ローカル全アクターの投稿、リプライ含む）・「グローバル」（`posts`テーブルの全投稿）を追加。バックエンドは`PostRepository::social_timeline`（`home_timeline`のLATERAL方式候補と`local_timeline`の`is_local`候補をUNIONしてから外側で再度LIMIT）・`global_timeline`（`local_timeline`から`is_local`条件のみ外したもの）を新設し、カスタムAPI（`GET /api/notes/social-timeline`・`/global-timeline`）とMisskey互換API（`POST /api/notes/hybrid-timeline`・`/global-timeline`、Ariaからの呼び出し用）の両方から利用できる。新規テーブル・マイグレーションなし。詳細: `docs/protocols.md` 7節、`docs/database.md`、`docs/ui_spec.md` 2.4b/2.4d/2.4e節
- [x] **Misskey互換APIの本文内カスタム絵文字（#88, #156）** — Noteレスポンスの`emojis`へ本文shortcodeと画像URLの対応を返し、Aria等のMisskeyクライアントで画像表示できるようにする。ActivityPub投稿では絵文字情報がactor側mapに保持される場合があるため、投稿・actor両方のmapを統合する。
- [x] **投稿本文カスタム絵文字の経路間整合（#126）** — ローカル・AP・Bluesky各受信経路で本文shortcodeの`emoji_map`を解決し、AP送信では保存済みmapから標準Emoji tagを付与する。欠落tagは同一リモートドメインの絵文字カタログで補完し、ZIPインポート時もshortcode文字種を検証する。
- [x] **引用ポストの共通表示（#116）** — Fedibird/MisskeyのAP引用フィールドとBsky record embedを`quote_of_post_id`へ統合し、既存3形式をbackfill。APIは可視性を守って引用元を1段だけ埋め込み、NoteCardは返信・CW本文・時刻・添付・アンケート・リアクション・「引用あり」を枠付きカードに表示する。
- [x] **引用投稿機能（#134）** — 投稿カードからローカル・Fedi・Bsky投稿を引用するコンポーザを開き、Fedi/Bskyへ配送できる。Fedi配送はMisskey互換引用（Bsky元はbsky.app URL）、Bsky配送はネイティブ引用（Fedi元はメタデータ付きURLカード）を使い分ける。
- [x] **クロスプロトコル・リポストのURLカード化（#132）** — Fediリモート投稿をBskyへリポスト配送する際、代替本文を「🔁」のみにして元投稿URLをexternal embedカードとして添付する。カードには元投稿者名（ID）・投稿本文・先頭画像（画像なしなら投稿者アイコン）のサムネイルも設定する。
- [x] **Bluesky GIF添付の受信（#160）** — `app.bsky.embed.external` のTenor/Klipy GIF URLをBluesky配信用MP4/WebM URLへ変換し、通常の動画添付として保存・表示する。
- [x] **BskyのURLカード（external embed）表示** — GIFピッカー由来を除く`app.bsky.embed.external`の`url`/`title`/`description`/`thumb`を`post_link_cards`へ保存し、フロントで`embedSrc`の有無で埋め込みプレーヤー/x.com/一般URLの3種のカード表示（`LinkCard`）に振り分ける（対応サービスはoEmbed discovery＋管理者ホワイトリストで決まる）。x.comはクリック時に公式`widgets.js`でツイートをライブ埋め込みする。詳細: `docs/database.md`、`docs/protocols.md`、`docs/ui_spec.md`
- [x] **FediのURLカード表示（複数枚対応）** — APにはembed概念が無いため、本文中のMarkdownリンクからURLを抽出（最大5件、画像記法・ハッシュタグリンクは除外）し、`Job::OgpFetch`でOGPを非同期取得する。SSRF対策の共通フェッチ関数を`seiran-api`から`seiran_common::net`へ移動し、`/proxy`・リモート絵文字インポートと共有。`posts.link_card_*`（単一カラム）は`post_link_cards`（`post_id`+`position`で複数保持）へ統合し、Bsky側もこちらへ移行。フロントは`Note.linkCards`配列を`LinkCard`でmapして複数枚を縦に並べる。詳細: `docs/database.md`、`docs/protocols.md`、`docs/ui_spec.md`
- [x] **URLカード埋め込みプレーヤーのoEmbed discovery化** — 個別サイトのハードコード（YouTube動画ID抽出・Spotify/Apple Musicのembed URL組み立て）を廃止し、oEmbed discovery（`<link rel="alternate" type=".../json+oembed">`検出→JSON取得→`html`からiframe src抽出、`net::fetch_ogp`がOGPと同じページ取得で処理）＋管理者設定ホワイトリスト（`site_settings.oembed_allowed_domains`、改行区切り、各行「domain」または「domain,oembedエンドポイントURL」、後方一致、TTL 60秒キャッシュ）方式に統一。既定でYouTube/Spotify/Apple Music/SoundCloudはHTML discovery経由、Vimeoは discoveryタグが無いため固定エンドポイント指定で許可される。管理者はドメイン（＋必要なら固定エンドポイント）を1行追加するだけで新サービスに対応できる。Bsky受信投稿（`app.bsky.embed.external`にはiframe情報が無い）は非同期`Job::LinkCardEmbedResolve`が後追いで`embed_src`を解決する。x.comは対象外（`widgets.js`方式を維持）。詳細: `docs/database.md`、`docs/protocols.md`、`docs/ui_spec.md`
- [x] **GIFアニメの自動再生統一** — Bskyのアニメーション付き動画添付には、Tenor/Klipy GIFピッカー由来（`app.bsky.embed.external`をCDN動画URLへ変換）と、GIFファイル直接アップロード由来（`app.bsky.embed.video`に`presentation:"gif"`が付与される新経路、従来は通常動画として保存され再生ボタンを押すまで静止画に見えていた）の2系統がある。両方を`post_attachments.is_gif`で統一的にフラグ立てし、フロント`HlsVideo`の`isGif` propで自動再生・ミュート・ループ・コントロール無し表示に揃えた。既存のTenor/Klipy由来行はURLパターンでバックフィル。詳細: `docs/database.md`、`docs/protocols.md`、`docs/ui_spec.md`
- [x] **LTL/GTLの公開範囲修正（#91）** — フォロワーで閲覧権限があっても、フォロワー限定投稿はローカル/グローバルタイムラインへ表示せず、ホーム/ソーシャルタイムラインにだけ表示する。
- [x] **タイムライン選択タブの永続化（#90）** — 最後に選択したホーム/ローカル/ソーシャル/グローバル/リスト/ハッシュタグをLocalStorageへ保存し、リロード後に復元する。
- [x] **リモート絵文字インポート（#73）** — AP受信（投稿本文・表示名・絵文字リアクション）で見つけたカスタム絵文字を`remote_emojis`テーブルへカタログ化し、管理画面「カスタム絵文字」パネルの「リモート」タブ（検索・インポートボタン）、およびNoteCard本文・絵文字リアクションの右クリックメニュー（管理者のみ）の2経路から、カテゴリ・タグ・ライセンスを指定してローカルの`custom_emojis`へ取り込めるようにした。画像取得は既存のメディアプロキシのSSRF対策ロジック（`fetch_validated`として共通化）を再利用する。詳細: `docs/architecture.md`、`docs/database.md`、`docs/ui_spec.md` 2.8節
- [x] **pg_bigmによる検索のパフォーマンス向上（#97）** — PostgreSQL 16へpg_bigmを組み込む専用Dockerイメージを追加し、投稿本文の部分一致検索を`LOWER(body) LIKE LOWER(...)`とbigm GINインデックスの組み合わせへ移行。アクターのサジェスト検索は対象外。詳細: `docs/architecture.md`、`docs/database.md`
- [x] **用途別ユーザー検索の最適化** — リスト編集・DMの検索は表示名と全ID表記をまとめたpg_bigm部分一致、投稿欄のメンション候補は2本のB-tree式インデックスによるハンドル前方一致へ分離し、ローカル候補は入力中の短縮/Fedi/Bsky形式に合わせて返す。詳細: `docs/architecture.md`、`docs/database.md`、`docs/ui_spec.md` 2.1/2.7節
- [x] **Bluesky AppView互換の検索式（#101）** — 引用句・AND/OR/NOT・括弧補正と`from:`・`mentions:`・`domain:`・`since:`・`until:`をローカルDB検索にも適用。`lang:`はローカル/Fedi投稿の言語未宣言を考慮して常にTRUEとして扱う。詳細: `docs/architecture.md`

- [x] **投稿フォームのボタン再配置（#152）** — 配送先・公開範囲を従来の公開範囲バー位置へツールチップ付きアイコンボタンとして集約し、公開範囲を3連排他ボタン化。メディア添付をフォーム最下段の投稿ボタン左へ移し、Bluesky公式SVGを同梱。詳細: `docs/ui_spec.md` 2.4b節

- [x] **投稿フォームの公開範囲を送信ボタン自体に統合** — 公開範囲の事前選択＋エラーポップアップ方式をやめ、🌐投稿/🌙ひかえめ/🔒️プライベートの3つの投稿ボタンへ統合。Bluesky配送オン時は🔒️プライベートをグレーアウトし、ホバー/クリックで理由を説明する吹き出しを表示する。Fediverse配送アイコンは絵文字からFediverseロゴSVGへ変更し、NoteCard・プロフィール画面のFediverseプロトコル表示も同ロゴへ統一。メディア添付ボタン・残り文字数をFediverse/Bluesky配送ボタンと同じ行へ移設し自作の図画アイコンに変更、投稿フォーム内の各行コンテナへNoteCard同様の`min-width: 0`を徹底してスマホ幅でのフローティングボタン位置崩れを防止。詳細: `docs/ui_spec.md` 2.4b節

- [x] **投稿ボタンのCtrl+Enterデフォルトを明示化・矢印キーナビゲーション対応** — Ctrl+Enter等のショートカット送信先が打鍵するまで分からない問題に対応。送信先の投稿ボタンに5px幅の赤枠マーカーを表示し（ブラウザ既定の青いフォーカスアウトラインは`:focus-visible`で無効化して置き換え）、Tabフォーカス中はそのボタンへ追従する。通常投稿・引用は最後に送信した公開範囲・配送先トグルを`localStorage`（`seiran:composer-defaults`）へ記憶し次回のデフォルトにする（返信は親ポストから決まる専用のデフォルトのため対象外）。デフォルトボタンが公開範囲の相互排他でグレーアウトしている間にショートカット送信すると、意図しない公開範囲へ無言で送信せずパブリック投稿ボタンへフォーカスを移す。投稿ボタン列・操作ボタン列（Fedi/Bsky配送・添付・アンケート・CW）それぞれの中を左右矢印キーで巡回でき、投稿ボタン列からは上矢印でBsky配送ボタンへ、操作ボタン列からは上矢印で本文入力欄・下矢印でデフォルトの投稿ボタンへ移動する（`ComposerEditor`を`forwardRef`化して本文DOMを公開）。詳細: `docs/ui_spec.md` 2.4b節

- [x] **NoteCardアクション列の整理と返信・引用・リポスト・リアクション件数の表示** — 返信・引用・リポスト・リアクション・ケバブメニューの5ボタンをキャプション文言なしの同一体裁に統一し、リポスト済み/リアクション済みは枠線、リポスト不可は薄字で表現。`posts`テーブルへ`reply_count`/`quote_count`/`repost_count`の非正規化カウンタ列とDBトリガーを追加し、各アイコン横に件数（0件は非表示、1000以上はK、100万以上はM表記）を表示する。詳細: `docs/database.md`、`docs/ui_spec.md` 2.2b節

## 未完了・今後の課題

### フロントエンド

- [x] **言語切り替えUI** — 設定画面「表示」（`/settings/appearance`、#55）で自動/日本語/英語を選択可能。詳細は上記フェーズ7.16、`docs/ui_spec.md` 2.7節
- [x] **ポスト詳細画面の充実（#226）** — 右ペインを5タブ構成（リポストラッパーは元投稿者タブが増え6タブ）へ拡張。詳細: `docs/ui_spec.md` 2.3節
  - [x] 投稿者タブ（プロフィール概要＋固定ポスト、`AuthorPanel`。リポストラッパーはリポストした人自身が対象）
  - [x] 元投稿者タブ（リポストラッパーのみ、元投稿の書き手のプロフィール概要＋固定ポスト、既存タブの末尾に追加）
  - [x] 返信タブ（再帰的な返信・引用ツリー、`ReplyThreadPanel`。`GET /api/notes/:id/replies`は`WITH RECURSIVE`で`reply_to_post_id`/`quote_of_post_id`を辿る自前実装。真のMisskey APIワイヤー互換ではなく、再帰トラバーサルの考え方をMisskeyの`notes/children`に倣った内部API）
  - [x] 前後のポストタブの仕様調整（最大5件、タブを開くと同時に自動読み込み、対象ポストへ自動スクロール）
  - [x] タブシートを右ペイン上端にsticky固定、タブ選択状態のURL同期（リロード後も維持）、前後のポストのスクロール位置をポストIDごとに記憶しブラウザバックで復元
  - [x] リアクションタブの一覧化（絵文字×ユーザー一覧、`ReactionListPanel`）
  - [x] リポストタブ（`RepostListPanel`、取り消し済みも履歴として表示）
- [x] **ポスト詳細画面のログイン不要化** — `/notes/:id`から`RequireAuth`ガードを撤去。閲覧系API（`GET /api/notes/:id`・`/context`・`/replies`・`/reposts`・`/reactions/:content/actors`・`/api/users/profile`）は元々`MaybeAuthedUser`で未ログイン対応済みだったため、フロント側のルーティング変更のみで対応。未ログイン時は左メニュー最下部のユーザーチップが現在画面への`redirect`付き`/login`誘導ボタンに差し替わる。詳細: `docs/ui_spec.md` 2.3節
- [x] **ユーザープロフィール画面のログイン不要化** — `/:acct`（`/@handle`）・旧`/profile?q=`から`RequireAuth`ガードを撤去。未ログイン時は対ユーザー操作メニュー（フォロー/ミュート/ブロック/通報）の代わりにログイン誘導ガイダンス文を表示。詳細: `docs/ui_spec.md` 2.2節
- [ ] **ユーザー製翻訳ファイルの適用・配布機能** — ユーザーが独自の言語ファイル（`i18n/locales/{lng}/*.json` と同形式）を作成し、アプリに読み込ませて適用・配布できるようにする構想。現状の名前空間分割構成は `i18n.addResourceBundle()` によるこの拡張を見据えたもの

### プロトコル

- [ ] **ゼロトラストハンドシェイク（リモートseiranアクター専用検証）**
  - [ ] Bioの `seiran_signature: [ATP_DID]` パターン検出ロジック
  - [ ] 相手ドメインの `/.well-known/seiran/verify-actor` への検証リクエスト
  - [ ] 検証成功時の `actor_type = 'remote_seiran'` 昇格と `seiran_pair_actor_id` の相互紐付け
- [ ] **リモートseiran特権初期同期**
  - [ ] `/api/seiran/v1/posts/export` エンドポイント
  - [ ] 相手サーバーからの生データ一括インポート（最大300件）
- [ ] **他seiranサーバー間マージの ATP 経路対応** — `seiran_post_uuid` を Bsky レコード本体にも埋め込み、Jetstream経由で先に取り込まれた投稿ともマージできるようにする（`docs/protocols.md` 5節の既知の制約）
- [ ] **`actor_metadata_resolve` ジョブの実装** — 現状ハンドラはスタブ、enqueueする箇所も無い。`/verify-actor` ハンドシェイク検証・Webfinger解決・アバター等のキャッシュを実処理として実装する
- [ ] **`inbound_activity_process` のドメイン単位レート制限**
- [ ] **トレンド集計** — バックエンド未着手（フロントエンドはプレースホルダのみ表示）
- [ ] **ユーザー設定に「Bsky DM受信許可」項目を追加** — 現状 `chat.bsky.actor.declaration` の `allowIncoming` は登録時・バックフィルとも `"all"` 固定でコミットする（`docs/protocols.md` 9節）。ユーザーが `"all"`/`"following"`/`"none"` を選べる設定画面UIとAPIを追加する
- [ ] **リアクション一覧表示でのブロック/ミュート除外** — `fetch_reactions_map` は対象外（`docs/protocols.md` 10節）
- [ ] **公開リストタイムラインのブロック/ミュートフィルタリング** — `list.rs::timeline` は「閲覧者情報を持たない」設計のため未対応。対応するには閲覧制御全体の見直しが必要（`docs/protocols.md` 10節）
- [x] **AT Protocol PDS 読み取り・同期系エンドポイント拡充** — `com.atproto.repo.listRecords`/`describeRepo`、`com.atproto.sync.listRepos`/`getLatestCommit`/`listBlobs`。詳細: `docs/protocols.md` 3節
- [x] **AT Protocol PDS 書き込み系エンドポイント** — `com.atproto.repo.createRecord`/`putRecord`/`deleteRecord`/`applyWrites`（`app.bsky.feed.post`以外の任意コレクション。`createSession`のaccessJwtで認証）。詳細: `docs/protocols.md` 3節
- [x] **AT Protocol PDS セッション認証系エンドポイント** — `com.atproto.server.createSession`/`refreshSession`/`deleteSession`/`getSession`/`createAppPassword`/`listAppPasswords`/`revokeAppPassword`。詳細: `docs/protocols.md` 3節
- [x] **AT Protocol PDS XRPCプロキシ（`atproto-proxy`ヘッダー）** — `app.bsky.feed.getTimeline`/`searchPosts`/`app.bsky.notification.listNotifications`等のAppView専用メソッドをAppViewへ透過転送する。詳細: `docs/protocols.md` 3節
- [x] **生年月日プロフィール項目（Misskey互換`birthday`、AP `vcard:bday`連合、ATP `personalDetailsPref`同期）** — `actors.birth_date`/`birth_date_public`。詳細: `docs/protocols.md` 3節、`docs/database.md`

### インフラ・パフォーマンス

- [ ] **`RedisSessionStore`** — 検索セッションのRedis保存（現状InMemoryのみ、スケールアウト時に必要）
- [ ] **Turnstile 自然人判別**（優先度: 低） — `TURNSTILE_SECRET_KEY` 設定時のみ有効化、登録/ログイン/パスワードリセットでの検証

### サードパーティクライアント互換

- [x] **Misskeyストリーミングのチャンネル購読対応**（homeTimeline/localTimeline/hybridTimeline/globalTimeline/userList/hashtag）
- [ ] **フロントエンドのMisskeyスキーマへの追従改修**、検証済み旧カスタムエンドポイントの整理
- [ ] APIレスポンスの `bio` 末尾に本尊URLを自動挿入するフォールバック（ZonePane/Miria/Aria等の非Misskey互換画面向け）
- [x] **`visibility` の値語彙をMisskey本家（`public`/`home`/`followers`/`specified`）にマッピング**。詳細: `docs/protocols.md` 7節

### テスト・QA

- [ ] 重複排除（シナリオ2マージ処理）のユニットテスト
- [ ] 未来補正タイムスタンプ採番のテスト
- [x] 検索ブレンドアルゴリズムの挙動テスト
- [ ] 連合（Federation）統合テスト（モックAP/ATPサーバー、他seiranハンドシェイク・特権同期のテスト）
- [x] **`e2e/tests/home-feed-state.spec.ts`「選択タブとスクロール位置が保持される」のflaky対策** — スクロール位置が200pxを超えた瞬間の値ではなく、値が安定してから比較するよう変更（ブラウザのscroll anchoring等で復元直後にわずかに動くケースを許容する）。
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore` 環境での動作確認、プロダクションビルド・デプロイ手順の検証）
- [x] Playwright E2E基盤の構築（`e2e/`、スタブPLCサーバー、E2E専用DB）と新規登録フローの疎通テスト
- [x] PR CIでfrontendユニットテストとPlaywright E2E全件を実行し、E2E失敗時のtraceをartifactとして保存（#98）
- [x] Rust全体へrustfmtを適用し、`cargo fmt --check`と警告ゼロのfrontend lintをCIで強制（#136）
- [x] E2Eテストの拡充（ログイン、投稿、フォロー、返信、リアクション、検索、プロフィール編集、ハッシュタグ）
- [x] Fedi配送のE2E化（投稿・返信・リポストがacceptedフォロワーのinboxへ正しいアクティビティで配送されることを、スタブFediアクター＋実HTTP Signaturesで検証）
- [x] フロントエンドのユニットテスト基盤（vitest + jsdom）を導入し、`lib/format.ts`・`lib/reaction.ts`・`lib/richTextPatterns.ts`・`api/client.ts`（`getErrorMessage`/`cursorParams`/`throwIfError`/`parseJsonBody`）・`NoteCard`/`PostComposer`内の純関数にテストを追加（`npm test`）
- [x] 管理画面（`/admin`）のE2E化（アクセス制御、サイト設定変更・永続化確認、ユーザー凍結/凍結解除）
- [x] リスト機能（`/settings/lists`）のE2E化（作成・改名・メンバー追加/削除・削除）
- [x] クイック通知（ホーム右ペイン`NotificationsPanel`）のE2E化（他ユーザーのリアクションがWS経由でリアルタイムに一覧へ反映されることを検証）。`NotificationsPage`（`/notifications`）は中央ペインに`NotificationsPanel`を表示する形で実装済み、専用ページへ直接遷移した場合の表示もE2Eで検証
- [x] ピン留め・リポスト取消のUI側状態変化のE2E化（ボタン表示のトグル確認）
- [x] Bsky側の配送E2E（リモートBskyアクターからのフォロー受理をポーリング方式（`getFollowers`、`seiran-atp-repo::bsky_follower_poll`）で検知し、投稿の`subscribeRepos`配送までを通しで検証）
- [x] メンション通知のE2E化（ローカル投稿・Fedi受信）。ローカルは`@username`投稿で相手に通知が届くこと・自己メンションで通知されないことを検証、Fedi受信はスタブFediアクターから`tag[].type=="Mention"`付きCreateを送りメンション通知が届くことを検証（`e2e/tests/notifications.spec.ts`）
- [ ] Bsky受信のメンション通知のE2E化 — `seiran-atp-repo::firehose`は本物のJetstreamサーバーへ接続する設計で、E2E側にイベント注入用のモックが無いため現状のE2E基盤では自動テストできない。実装（`save_bsky_post`内の通知処理）とcurlでの手動確認のみ
- [x] プロフィール画面のフォロー中/フォロワータブのE2E化（#56、`e2e/tests/follow.spec.ts`）。ユーザー間フォロー後、双方のプロフィールでフォロー数/フォロワー数バッジから右ペインのタブが切り替わり相手アクターが一覧表示されることを検証
- [x] 設定画面のE2E化（#55、`e2e/tests/settings.spec.ts`）。設定メニューからアカウント設定への遷移とDID表示、現在パスワード誤り時のエラー表示から正しいパスワードでの変更成功・新パスワードでのログインまでの一連、ミュート・ブロック一覧の表示とタブ切り替え・解除操作、表示設定での言語切り替え（英語選択→保存確認→自動に戻す→`/api/auth/me`の`language_preference`検証）を検証

既存の結合テスト基盤: `crates/seiran-api/tests/`（実DB + 実 `seiran_api::router` を使用、`#[ignore]` で通常の `cargo test` から除外し `cargo test -p seiran-api --test <name> -- --ignored` で明示実行）。

- [x] **リポスト・引用通知 (#198)** ローカルユーザーの投稿を他ユーザーがリポスト／引用したとき、`type="repost"` / `type="quote"` の通知を永続化してリアルタイム反映する。自己操作とリモート投稿宛は除外し、通知から新しいリポスト／引用投稿へ遷移できる。詳細: `docs/protocols.md` 8節、`docs/ui_spec.md`
- [x] **認証・ユーザー操作レート制限（#223）** ログイン/TOTPの資格情報種類数制限（ログイン成功でリセット）、IP自動ブロックと管理UI、ログイン・登録全フローへのTurnstile連携・IP別登録数制限、ロール別の投稿数・新規フォロー数・リスト作成数/最大人数・検索回数・メンション宛先数・メディア容量制限を追加。E2E: `e2e/tests/rate-limit.spec.ts`。詳細: `docs/architecture.md`、`docs/database.md`、`docs/ui_spec.md`
- [x] **Fedi投稿のHTML構造を保持したリッチ表示（#233）** — 受信AP `Note.content`の`<blockquote>`/`<ruby>`/`<b>`/`<i>`/`<s>`/`<code>`/`<pre>`等がプレーンテキスト化（`ap_content_to_markdown_body`）で失われる不具合を修正。`body`はMisskey互換API・Bsky配送・検索・ハッシュタグ抽出が前提とする唯一のフォーマットとして無変更のまま維持し、allowlistでサニタイズしたHTMLを新規`posts.content_html`（リモートFedi投稿のみ）に持たせる方式で追加。メンション/ハッシュタグの`<a>`はseiran内部の遷移先へ書き換え、それ以外の意味的構造は保持する。MFMの装飾関数（`spin`/`jelly`/`blur`等）はMisskey側変換時点で全て`<i>`に縮退し相互に区別できないためそのまま表示。フロントは`RichHtml`コンポーネント（無ければ従来の`RichText`にフォールバック）で描画する。詳細: `docs/database.md`、`docs/protocols.md` 6節
- [x] **ホームタイムラインのリプライ先フォロー条件** — フォロー中ユーザーの投稿は無条件で表示していたが、リプライ投稿についてはリプライ先投稿者もフォロー中（または自分自身）であることを追加条件にした。SQL関数`post_reply_target_followed(viewer_id, reply_to_post_id)`に判定を集約し、REST（`home_timeline`/`social_timeline`のフォロー中パート、ローカル全体パートは対象外）とWebSocket（`FollowRepository::find_home_recipient_ids`によるホームタイムライン新規投稿配信）の両方から共有する。E2E: `e2e/tests/timeline-visibility.spec.ts`、`e2e/tests/streaming-channels.spec.ts`。詳細: `docs/database.md`、`docs/protocols.md`

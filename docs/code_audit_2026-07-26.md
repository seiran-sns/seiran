# コード改善大会 現行コード監査（#98、2026-07-26）

対象: `main` の `1f20482`。過去の改善レポートで対応済みの項目は現行コードで
再確認し、未解消または今回新たに判明した事項だけを記録する。

## 結論

### リファクタリング・テスト

- CIのFrontend jobはtypecheck/lintまでで、既存Vitest 86件を実行していなかった。
  `npm test`を必須化した。
- Playwright 67件は共有DBを使う一方、specファイルが2 workerで並行実行されていた。
  `fullyParallel: false`はファイル間並列を止めないため、1 workerへ固定した。
- クリーン環境のRust初回ビルドは実測5分52秒で、backend起動上限3分を超えていた。
  backend起動上限を10分、CI job全体を30分とした。
- E2Eのbackendに`FRONTEND_ORIGIN`がなく、Docker用既定値
  `http://frontend:5173`へ接続して直リンク（`/notes/:id`、`/@user`）が502になっていた。
  E2E専用Viteへ明示的に向けた。
- CIのNode.js 20ではTypeScriptファイルを直接実行できないため、E2Eの補助サーバーと
  DB起動待ちは`tsx`を介して実行するよう統一した。
- HomePage離脱時、scroll eventの保存を遅延する`requestAnimationFrame`がunmountでcancelされ、
  最終スクロール位置を失う競合があった。保存先は再renderを起こさないref内Mapなので、
  scroll eventで即時保存するよう修正した。加えて、active feed tabの
  `scrollIntoView`がタブの横位置だけでなくwindowを縦に0へ戻していたため、
  タブコンテナ自身の横`scrollTo`へ限定した。Router遷移のDOM差し替えがscroll eventより
  先になる場合にも備え、AppShellのclick captureで遷移前の位置を保存する。
- CIへE2E jobを追加し、失敗時のPlaywright traceをartifactとして7日保存する。
- S3 stubを使うspecが終了時にstorage providerをactiveのまま残し、後続specが停止済みstubを
  選んでいた。各testの`finally`でproviderをinactive化し、ファイル間の状態汚染を解消した。

### RDB・Webパフォーマンス

- ルート画面と無関係な管理・設定・認証ページまで初期bundleへ含まれ、production buildの
  main JSが約1,013 kB（gzip約306 kB）だった。状態保持が必要なHomePageは同期のまま、
  その他のページを`React.lazy`でroute単位に分割し、main JSを約570 kB
  （gzip約177 kB）へ削減した。
- 直近の#97/#111で投稿・actor検索のpg_bigm/indexと用途別limitが整備済みであり、
  今回の監査で即時修正すべき新たな無制限一覧queryは確認しなかった。
- キャッシュ候補として、公開site settings、トレンド、リモートActor公開鍵がある。
  ただしsite settingsは管理画面で即時反映が期待され、公開鍵は既にプロセス内cacheを持つ。
  更新遅延とinvalidaton複雑化に対する実測値がないため、推測だけでcacheは追加しない。
  endpoint別のp95/DB query時間を計測してからTTLを決める。

### セキュリティ

- パスワードリセットは「有効性SELECT→password更新→used_at更新」が別queryで、
  同一tokenの並行使用が可能だった。`UPDATE ... RETURNING`によるtoken消費とpassword更新を
  1 transactionへ統合した。
- `npm audit`で旧依存に9件（high 6/moderate 3）を確認した。Vite、ESLintと関連pluginを
  現行安全版へ更新し、highを0件にした。React Routerは7系への移行に互換性検証が必要なため
  6系最新を維持し、client-side SPAでは使わないSSR経路のmoderate 2件だけが残る。
- media proxyのDNS再解決を含むprivate/link-local拒否、MiAuth callback scheme/host検証、
  secret暗号化、gitleaks CIは現行コードで実装済みであることを再確認した。

### UI・API共通化

- ページ遷移の共通境界を`Suspense`へ統一し、全routeを同じlazy-loading方式へ揃えた。
- NoteCard/NoteHoverPreviewや右ペイン/中央ペインには似た表示があるが、操作可能性と
  情報密度が異なる。多少の見た目差だけでなく挙動差も大きく、現時点での単一component化は
  条件分岐を増やすため見送る。
- frontend向けRESTとMisskey互換APIはDTO・pagination・error契約が異なり、単純統合は
  互換APIの破壊につながる。共通化すべきrepository/read-modelは既に共有されているため、
  endpoint統合は新規API追加時にMisskey独自拡張を優先する方針とする。

## 検証

- frontend: typecheck、ESLint、Vitest 86件、production build
- Rust: build、workspace clippy（warnings deny）、workspace test
- E2E: Playwright 67件（隔離PostgreSQL・PLC/AppView stub）

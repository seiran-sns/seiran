//! api ロールの axum ルーター定義。領域ごとのサブルーター（`*_routes`）を`router`で合成する。
//! 同じパスを複数のサブルーターに
//! 分けて定義しないこと（axum の`merge`は同一パスの重複でパニックする）。

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post},
    Router,
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use crate::{handlers, middleware, AppState};

/// Mastodon 互換 API のうち、ブラウザ上の Web クライアント（Elk・Phanpy 等、任意のオリジン）
/// から直接叩かれるパス。認証は `Authorization: Bearer` だけで Cookie を使わないので、
/// `/xrpc/*` と同じくオリジン制限の対象外にする。SPA 専用の `/api/oauth/*`（承認画面）は含めない。
fn is_mastodon_public_path(path: &str) -> bool {
    path.starts_with("/api/v1/")
        || path.starts_with("/api/v2/")
        || path == "/oauth/token"
        || path == "/oauth/revoke"
}

/// CORS 設定。
fn cors_layer(state: &AppState) -> CorsLayer {
    // [SEC-2] `frontend_origin`（`FRONTEND_ORIGIN`環境変数）と自ドメインのみ許可する。
    // `local_domain`は初回セットアップ完了まで未確定（`OnceLock`）なため、CORSレイヤ構築時
    // ではなくリクエストごとに評価する`predicate`を使う（起動時点の値を静的に焼き込むと、
    // セットアップ完了前後で判定がずれる）。認証は`Authorization`ヘッダー方式で
    // Cookieを使わない（`allow_credentials`は付与していない）ため古典的CSRFは成立しないが、
    // `Any`のままだと任意サイトのJSが公開APIを無制限に叩ける。
    //
    // `/xrpc/*`・`/.well-known/*`はAT Protocol標準のXRPCエンドポイントで、bsky.app等の
    // 外部ATクライアントがブラウザから直接叩くことを前提とした公開APIのため、この
    // オリジン制限の対象外とする（公式Bluesky PDSも`Access-Control-Allow-Origin: *`を返す）。
    // これが無いとbsky.appのログイン画面で「サービスに接続できません」となり、ATクライアント
    // からのアクセスが一切成立しない。
    let frontend_origin = state.frontend_origin.clone();
    let local_domain = state.local_domain.clone();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, req| {
            let path = req.uri.path();
            if path.starts_with("/xrpc/")
                || path.starts_with("/.well-known/")
                || is_mastodon_public_path(path)
            {
                return true;
            }
            let Ok(origin_str) = origin.to_str() else {
                return false;
            };
            if origin_str == frontend_origin {
                return true;
            }
            let local_domain = local_domain.as_str();
            origin_str == format!("https://{}", local_domain)
                || origin_str == format!("http://{}", local_domain)
        }))
        .allow_methods(Any)
        // ヘッダーも `Any`（ワイルドカード）にする。bsky.app等の外部ATクライアントが送ってくる
        // カスタムヘッダー（`atproto-proxy`/`atproto-accept-labelers`/`x-bsky-topics`等）を
        // 個別に列挙すると、新しいヘッダーが増えるたびにプリフライトで弾かれる。
        // `allow_credentials`を付与していない（Cookie不使用）ため、ヘッダーを`Any`にしても
        // ブラウザのCORS仕様上安全（`Access-Control-Allow-Headers: *`は非credentialedリクエストで
        // のみ有効、credentialed併用時のみ禁止される組み合わせ）。
        .allow_headers(Any)
}

/// 管理系ルート。ロールごとに専用ルータへ分割し、`route_layer`で認可を強制する（#221）。
fn admin_routes(state: &AppState) -> Router<AppState> {
    // 管理系ルートはロールごとに専用ルータへ分割し、`route_layer`で認可を強制する（#221）。
    // ハンドラ側では認可チェックを一切行わない（呼び忘れによる無認可到達を構造的に防ぐ）。
    let admin_router = Router::new()
        .route(
            "/api/admin/storage-providers",
            get(handlers::admin::storage::list_storage_providers)
                .post(handlers::admin::storage::create_storage_provider),
        )
        .route(
            "/api/admin/storage-providers/:id",
            patch(handlers::admin::storage::update_storage_provider)
                .delete(handlers::admin::storage::delete_storage_provider),
        )
        .route("/api/admin/users", get(handlers::admin::users::list_users))
        .route(
            "/api/admin/users/:id/suspend",
            post(handlers::admin::users::suspend_user),
        )
        .route(
            "/api/admin/users/:id/unsuspend",
            post(handlers::admin::users::unsuspend_user),
        )
        .route(
            "/api/admin/users/:id/role",
            post(handlers::admin::users::change_user_role),
        )
        .route(
            "/api/admin/users/:id/totp/disable",
            post(handlers::admin::users::disable_user_totp),
        )
        .route(
            "/api/admin/site-settings",
            get(handlers::admin::site_settings::get_site_settings)
                .patch(handlers::admin::site_settings::update_site_settings),
        )
        .route(
            "/api/admin/relays",
            get(handlers::admin::relays::list_relays).post(handlers::admin::relays::create_relay),
        )
        .route(
            "/api/admin/relays/:id",
            delete(handlers::admin::relays::delete_relay),
        )
        .route(
            "/api/admin/auth-ip-blocks",
            get(handlers::admin::auth_ip_blocks::list_ip_blocks),
        )
        .route(
            "/api/admin/auth-ip-blocks/:ip",
            delete(handlers::admin::auth_ip_blocks::unblock_ip),
        )
        .route(
            "/api/admin/rotation-key-backfill",
            post(handlers::admin::rotation_key_backfill::run_rotation_key_backfill),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::admin_only,
        ));

    let emoji_admin_router = Router::new()
        .route(
            "/api/admin/emojis",
            get(handlers::admin::emojis::list_emojis).post(handlers::admin::emojis::create_emoji),
        )
        .route(
            "/api/admin/emojis/:id",
            patch(handlers::admin::emojis::update_emoji)
                .delete(handlers::admin::emojis::delete_emoji),
        )
        // 絵文字インポート（#50）。多数のカスタム絵文字を含むZIPは数十〜数百MBになりうるため、
        // axum のデフォルトボディ上限（2MB）を明示的に引き上げる。
        .route(
            "/api/admin/emojis/import",
            post(handlers::admin::emoji_import::start_import)
                .layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/api/admin/emojis/import/:job_id",
            get(handlers::admin::emoji_import::get_import_status),
        )
        .route(
            "/api/admin/emojis/remote",
            get(handlers::admin::remote_emojis::list_remote_emojis),
        )
        .route(
            "/api/admin/emojis/remote/import",
            post(handlers::admin::remote_emojis::import_remote_emoji),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::emoji_admin_only,
        ));

    let report_moderator_router = Router::new()
        .route(
            "/api/admin/reports",
            get(handlers::admin::reports::list_reports),
        )
        .route(
            "/api/admin/reports/:id/close",
            post(handlers::admin::reports::close_report),
        )
        .route(
            "/api/admin/reports/:id/comments",
            get(handlers::admin::reports::list_comments)
                .post(handlers::admin::reports::add_comment),
        )
        .route(
            "/api/admin/reports/:id/delete-post",
            post(handlers::admin::reports::delete_subject_post),
        )
        .route(
            "/api/admin/reports/:id/suspend-user",
            post(handlers::admin::reports::suspend_subject),
        )
        .route(
            "/api/admin/reports/:id/forward",
            post(handlers::admin::reports::forward_report),
        )
        .route(
            "/api/admin/suspended-actors",
            get(handlers::admin::actors::list_suspended),
        )
        .route(
            "/api/admin/actors/:id/suspend",
            post(handlers::admin::actors::suspend_actor),
        )
        .route(
            "/api/admin/actors/:id/unsuspend",
            post(handlers::admin::actors::unsuspend_actor),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::report_moderator_only,
        ));

    Router::new()
        .merge(admin_router)
        .merge(emoji_admin_router)
        .merge(report_moderator_router)
}

/// ヘルスチェック・favicon・PWA manifest・画像配信・メディアプロキシ・初回セットアップ・通報・ドライブ。
fn base_routes() -> Router<AppState> {
    Router::new()
        // ヘルスチェック（外形監視用、認証不要）
        .route("/health", get(handlers::health::health))
        // サイトアイコンを favicon として返す（#42）
        .route("/favicon.ico", get(handlers::favicon::favicon))
        // PWA Web App Manifest（サイト設定から動的生成）
        .route("/manifest.webmanifest", get(handlers::manifest::manifest))
        // サイトアイコンのリサイズ配信（favicon/PWAアイコン共通）
        .route(
            "/api/site-icon/:sha256/:size",
            get(handlers::site_icon::site_icon),
        )
        .route(
            "/api/avatars/:actor_id",
            get(handlers::avatar::fallback_avatar),
        )
        // Misskey互換メディアプロキシ（リモート画像のCORS回避、SSRF防止付き）。
        // 本家Misskeyの `/proxy/:url*` は末尾に出力フォーマットのヒント（例: `image.webp`）を
        // パスセグメントとして付与できる仕様で、Aria等はこれを使い `{mediaProxyUrl}/image.webp?url=...`
        // という形式でリクエストする。seiranは`url`クエリパラメータのみで画像を
        // 解決するため、末尾のパスセグメントは無視してよい（フォーマット変換自体は行わない）。
        .route("/proxy", get(handlers::media_proxy::proxy))
        .route("/proxy/*rest", get(handlers::media_proxy::proxy))
        // セットアップ（初回管理者作成）
        .route("/api/setup/status", get(handlers::setup::setup_status))
        .route("/api/setup", post(handlers::setup::setup))
        // ユーザー通報
        .route("/api/reports", post(handlers::reports::create_report))
        // ドライブ（メディアアップロード）。動画・音声添付を考慮し 100MB まで許可
        // （axum のデフォルトボディ上限は小さいため明示的に上書きする）。
        .route(
            "/api/drive/files/create",
            post(handlers::drive::create_drive_file)
                .layer(DefaultBodyLimit::max(105 * 1024 * 1024)),
        )
        // 音声・動画の簡易視聴ページ（Bskyの外部リンクカードの参照先。直リンクだと
        // ダウンロードになってしまうため<audio>/<video>タグのみのHTMLを返す）
        .route(
            "/api/media/:media_file_id/watch",
            get(handlers::drive::watch_media),
        )
}

/// 認証・登録・既存DID転入・アカウント設定。
fn auth_account_routes() -> Router<AppState> {
    Router::new()
        // 認証
        .route(
            "/api/auth/verify-email",
            post(handlers::email_verify::request_email_verification),
        )
        .route(
            "/api/auth/verify-token",
            get(handlers::email_verify::verify_email_token),
        )
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/migration/start", post(handlers::migration::start))
        .route(
            "/api/migration/:id/submit-plc-token",
            post(handlers::migration::submit_plc_token),
        )
        .route(
            "/api/migration/:id/status",
            get(handlers::migration::get_status),
        )
        .route("/api/migration/:id/retry", post(handlers::migration::retry))
        .route(
            "/api/migration/:id/abandon",
            post(handlers::migration::abandon),
        )
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        .route(
            "/api/auth/request-password-reset",
            post(handlers::auth::request_password_reset),
        )
        .route(
            "/api/auth/verify-reset-token",
            get(handlers::auth::verify_reset_token),
        )
        .route(
            "/api/auth/reset-password",
            post(handlers::auth::reset_password),
        )
        // TOTP（二段階認証、#65）: ログイン2段階目・認証アプリ紛失時のメール解除
        .route("/api/auth/totp/verify", post(handlers::totp::totp_verify))
        .route(
            "/api/auth/totp/request-disable-email",
            post(handlers::totp::totp_request_disable_email),
        )
        .route(
            "/api/auth/totp/confirm-disable",
            post(handlers::totp::totp_confirm_disable),
        )
        .route(
            "/api/auth/passkeys/start",
            post(handlers::passkeys::authentication_start),
        )
        .route(
            "/api/auth/passkeys/finish",
            post(handlers::passkeys::authentication_finish),
        )
        // アカウント管理（退会等）
        .route("/api/account/withdraw", post(handlers::account::withdraw))
        .route(
            "/api/account/change-password",
            post(handlers::account::change_password),
        )
        .route(
            "/api/account/revoke-all-sessions",
            post(handlers::account::revoke_all_sessions),
        )
        .route(
            "/api/account/language",
            post(handlers::account::update_language),
        )
        .route(
            "/api/account/content-visibility",
            get(handlers::account::get_content_visibility)
                .post(handlers::account::update_content_visibility),
        )
        .route(
            "/api/account/lock",
            get(handlers::account::get_lock).post(handlers::account::update_lock),
        )
        .route(
            "/api/account/email/request-change",
            post(handlers::account::request_email_change),
        )
        .route(
            "/api/account/email/confirm-change",
            post(handlers::account::confirm_email_change),
        )
        .route(
            "/api/account/app-tokens",
            get(handlers::account::list_app_tokens).post(handlers::account::create_app_token),
        )
        .route(
            "/api/account/app-tokens/:id",
            delete(handlers::account::revoke_app_token),
        )
        // TOTP（二段階認証、#65）: 設定画面での有効化・無効化
        .route("/api/account/totp/status", get(handlers::totp::totp_status))
        .route("/api/account/totp/setup", post(handlers::totp::totp_setup))
        .route(
            "/api/account/totp/enable",
            post(handlers::totp::totp_enable),
        )
        .route(
            "/api/account/totp/disable",
            post(handlers::totp::totp_disable),
        )
        .route("/api/account/passkeys", get(handlers::passkeys::list))
        .route(
            "/api/account/passkeys/registration/start",
            post(handlers::passkeys::registration_start),
        )
        .route(
            "/api/account/passkeys/registration/finish",
            post(handlers::passkeys::registration_finish),
        )
        .route(
            "/api/account/passkeys/:id",
            delete(handlers::passkeys::delete),
        )
}

/// ノート（投稿・タイムライン・検索・DM・リアクション・ピン留め・スレッド）。
fn notes_routes() -> Router<AppState> {
    Router::new()
        // 投稿
        .route("/api/notes/create", post(handlers::notes::create_note))
        // 同じパスの POST は Misskey 互換 API（`notes_local_timeline`）。
        .route(
            "/api/notes/local-timeline",
            get(handlers::notes::local_timeline)
                .post(handlers::misskey::endpoints::notes_local_timeline),
        )
        .route(
            "/api/notes/home-timeline",
            get(handlers::notes::home_timeline),
        )
        .route(
            "/api/notes/social-timeline",
            get(handlers::notes::social_timeline),
        )
        // Misskeyクライアント（Aria等）は`/api/notes/global-timeline`をPOSTで叩く（#78）。GET/POST共存。
        .route(
            "/api/notes/global-timeline",
            get(handlers::notes::global_timeline)
                .post(handlers::misskey::endpoints::notes_global_timeline),
        )
        // Misskey 互換エイリアス
        // 同じパスの POST は Misskey 互換 API（`notes_home_timeline`）。
        .route(
            "/api/notes/timeline",
            get(handlers::notes::home_timeline)
                .post(handlers::misskey::endpoints::notes_home_timeline),
        )
        .route(
            "/api/notes/search",
            get(handlers::search::search_notes).post(handlers::misskey::endpoints::notes_search),
        )
        .route(
            "/api/notes/search-by-tag",
            post(handlers::misskey::endpoints::notes_search_by_tag),
        )
        .route("/api/open", post(handlers::open_target::open_target))
        // ダイレクトメッセージ（DM本体の送受信は既存の /api/notes/create を再利用する）
        .route("/api/dm/sessions", get(handlers::dm::sessions))
        .route(
            "/api/dm/sessions/:thread_root_id/messages",
            get(handlers::dm::thread_messages),
        )
        .route(
            "/api/dm/sessions/:thread_root_id/read",
            post(handlers::dm::mark_read),
        )
        .route("/api/dm/unread-count", get(handlers::dm::unread_count))
        // bsky宛DMメッセージの絵文字リアクション・「隠す」（fedi/localは既存の
        // /api/notes/:id/reactions・/api/notes/:id をそのまま使う、docs/protocols.md 9節）。
        .route(
            "/api/dm/messages/:id/reactions",
            post(handlers::dm_bsky_reactions::create_dm_bsky_reaction),
        )
        .route(
            "/api/dm/messages/:id/reactions/:content",
            delete(handlers::dm_bsky_reactions::delete_dm_bsky_reaction),
        )
        .route(
            "/api/dm/messages/:id/hide",
            post(handlers::dm_bsky_reactions::hide_dm_message),
        )
        .route("/api/streaming", get(handlers::streaming::streaming))
        .route(
            "/api/notes/:id",
            get(handlers::notes::get_note).delete(handlers::notes::delete_note),
        )
        .route(
            "/api/notes/:id/repost",
            delete(handlers::notes::delete_repost),
        )
        .route(
            "/api/reactions/frequent",
            get(handlers::notes::frequent_reactions),
        )
        .route(
            "/api/notes/:id/reactions",
            post(handlers::notes::create_reaction),
        )
        .route("/api/notes/:id/poll-vote", post(handlers::notes::vote_poll))
        .route(
            "/api/notes/:id/reactions/:content",
            delete(handlers::notes::delete_reaction),
        )
        .route(
            "/api/notes/:id/reactions/:content/actors",
            get(handlers::notes::reaction_actors),
        )
        .route("/api/notes/:id/pin", post(handlers::notes::pin_note))
        .route("/api/notes/:id/pin", delete(handlers::notes::unpin_note))
        .route(
            "/api/notes/:id/resolve-reference",
            post(handlers::notes::resolve_note_reference),
        )
        .route("/api/notes/:id/context", get(handlers::notes::note_context))
        .route("/api/notes/:id/replies", get(handlers::notes::note_replies))
        .route("/api/notes/:id/reposts", get(handlers::notes::note_reposts))
}

/// AP オブジェクト直リンク・OGP 付き HTML。
fn page_routes() -> Router<AppState> {
    Router::new()
        // ActivityPub Note / OGP注入済みSPA（Accept ヘッダーで振り分け、`handlers::ogp`）
        .route("/notes/:id", get(handlers::notes::get_note_ap))
        // Announce（リポストラッパー）canonical URL。ブラウザは /notes/:id へリダイレクト
        .route(
            "/announces/:id",
            get(handlers::notes::get_announce_redirect),
        )
        // プロフィールページ（OGP注入済みSPA HTMLを返す、`handlers::ogp`）
        .route("/@:handle", get(handlers::ogp::profile_ogp))
}

/// フォロー・ブロック・ミュート・リスト・ハッシュタグ・アクター検索・ユーザー。
fn social_routes() -> Router<AppState> {
    Router::new()
        // フォロー
        .route(
            "/api/follows/create",
            post(handlers::follows::create_follow),
        )
        .route(
            "/api/follows/delete",
            post(handlers::follows::delete_follow),
        )
        // 承認待ちフォロー（フォロー承認制、設定画面「承認待ちフォロー」）
        .route(
            "/api/follow-requests",
            get(handlers::follow_requests::list_follow_requests),
        )
        .route(
            "/api/follow-requests/count",
            get(handlers::follow_requests::count_follow_requests),
        )
        .route(
            "/api/follow-requests/:follower_actor_id/accept",
            post(handlers::follow_requests::accept_follow_request),
        )
        .route(
            "/api/follow-requests/:follower_actor_id/reject",
            post(handlers::follow_requests::reject_follow_request),
        )
        // フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）
        .route(
            "/api/account/follow-import",
            post(handlers::follow_import::start_import).get(handlers::follow_import::get_status),
        )
        .route(
            "/api/account/follow-import/cancel",
            post(handlers::follow_import::cancel_import),
        )
        // ブロック（Bsky準拠：フォロー強制解除＋相互完全非表示。Fediへは Block 配送、Bskyへは app.bsky.graph.block をコミット）
        .route("/api/blocks/create", post(handlers::blocks::create_block))
        .route("/api/blocks/delete", post(handlers::blocks::delete_block))
        .route("/api/blocks", get(handlers::blocks::list_blocks))
        // ミュート（ローカル効果のみ、AP/ATP配送なし）
        .route("/api/mutes/create", post(handlers::mutes::create_mute))
        .route("/api/mutes/delete", post(handlers::mutes::delete_mute))
        .route("/api/mutes", get(handlers::mutes::list_mutes))
        .route(
            "/api/repost-mutes/create",
            post(handlers::repost_mutes::create_repost_mute),
        )
        .route(
            "/api/repost-mutes/delete",
            post(handlers::repost_mutes::delete_repost_mute),
        )
        .route(
            "/api/repost-mutes",
            get(handlers::repost_mutes::list_repost_mutes),
        )
        // リスト（#63）
        .route(
            "/api/lists",
            get(handlers::lists::my_lists).post(handlers::lists::create_list),
        )
        .route(
            "/api/lists/:id",
            get(handlers::lists::get_list)
                .patch(handlers::lists::update_list)
                .delete(handlers::lists::delete_list),
        )
        .route("/api/lists/:id/members", post(handlers::lists::add_member))
        .route(
            "/api/lists/:id/members/:actor_id",
            delete(handlers::lists::remove_member),
        )
        .route(
            "/api/lists/membership/:actor_id",
            get(handlers::lists::list_membership),
        )
        .route(
            "/api/lists/:id/timeline",
            get(handlers::lists::list_timeline),
        )
        // ハッシュタグ
        .route(
            "/api/hashtags/pinned",
            get(handlers::hashtags::pinned_hashtags),
        )
        .route(
            "/api/hashtags/:name/timeline",
            get(handlers::hashtags::hashtag_timeline),
        )
        .route(
            "/api/hashtags/:name/pin",
            post(handlers::hashtags::pin_hashtag).delete(handlers::hashtags::unpin_hashtag),
        )
        .route(
            "/api/actors/search",
            get(handlers::actor_search::search_actors),
        )
        .route(
            "/api/actors/suggest",
            get(handlers::actor_search::suggest_actors),
        )
        // ユーザープロフィール
        .route(
            "/api/users/profile",
            get(handlers::users::user_profile).patch(handlers::users::update_profile),
        )
        .route("/api/users/posts", get(handlers::users::user_posts))
        // Misskey クライアント（Aria等）は同パスをPOSTで叩く（#81）。GET/POST共存。
        .route(
            "/api/users/following",
            get(handlers::users::user_following)
                .post(handlers::misskey::endpoints::users_following),
        )
        .route(
            "/api/users/followers",
            get(handlers::users::user_followers)
                .post(handlers::misskey::endpoints::users_followers),
        )
        .route(
            "/api/users/remote-follow-summary",
            get(handlers::users::user_remote_follow_summary),
        )
        // プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）
        .route(
            "/api/users/also-known-as",
            post(handlers::also_known_as::add),
        )
        .route(
            "/api/users/also-known-as/:actor_id",
            delete(handlers::also_known_as::remove),
        )
}

/// Misskey 互換レイヤー・MiAuth。
fn misskey_routes() -> Router<AppState> {
    Router::new()
        // Misskey 互換レイヤー
        .route("/api/meta", post(handlers::meta::api_meta))
        .route("/api/ap/show", post(handlers::misskey::endpoints::ap_show))
        .route(
            "/api/endpoints",
            post(handlers::misskey::endpoints::endpoints),
        )
        // カスタム絵文字一覧（未認証・Misskey クライアントのリアクションピッカー用）
        // Misskey 本家は `allowGet: true` でGET/POST両対応。Aria 等のクライアントは
        // POST で呼ぶため、GET のみだと 405 Method Not Allowed になり絵文字が出ない。
        .route(
            "/api/emojis",
            get(handlers::emojis::list_emojis).post(handlers::emojis::list_emojis),
        )
        // Misskey 準拠の追加エンドポイント（Phase 2）。既存のカスタムAPIと並存する。
        .route("/api/i", post(handlers::misskey::endpoints::api_i))
        .route(
            "/api/users/show",
            post(handlers::misskey::endpoints::users_show),
        )
        .route(
            "/api/users/notes",
            post(handlers::misskey::endpoints::users_notes),
        )
        .route(
            "/api/notes/show",
            post(handlers::misskey::endpoints::notes_show),
        )
        .route(
            "/api/notes/mentions",
            post(handlers::misskey::endpoints::notes_mentions),
        )
        .route(
            "/api/notes/reactions",
            post(handlers::misskey::endpoints::notes_reactions),
        )
        .route(
            "/api/notes/polls/vote",
            post(handlers::misskey::endpoints::notes_polls_vote),
        )
        .route(
            "/api/notes/hybrid-timeline",
            post(handlers::misskey::endpoints::notes_hybrid_timeline),
        )
        .route(
            "/api/notes/reactions/create",
            post(handlers::misskey::endpoints::reactions_create),
        )
        .route(
            "/api/notes/reactions/delete",
            post(handlers::misskey::endpoints::reactions_delete),
        )
        .route(
            "/api/notes/unrenote",
            post(handlers::misskey::endpoints::notes_unrenote),
        )
        .route(
            "/api/notes/user-list-timeline",
            post(handlers::misskey::endpoints::notes_user_list_timeline),
        )
        .route(
            "/api/following/create",
            post(handlers::misskey::endpoints::following_create),
        )
        .route(
            "/api/following/delete",
            post(handlers::misskey::endpoints::following_delete),
        )
        .route(
            "/api/i/notifications",
            post(handlers::misskey::endpoints::i_notifications),
        )
        .route(
            "/api/users/reactions",
            post(handlers::misskey::endpoints::users_reactions),
        )
        .route("/api/stats", post(handlers::misskey::endpoints::stats))
        // リストは既存機能（`handlers::lists`と共通の`ListRepository`）を返す実データ。
        // 他は未実装のMisskey機能（お知らせ・ハイライト・クリップ・ページ・Play・
        // ギャラリー）で、常に空配列を返すスタブ（#251、Aria非互換修正）。
        .route(
            "/api/announcements",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/featured-notes",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/clips",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/pages",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/flashs",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/gallery/posts",
            post(handlers::misskey::endpoints::empty_list_stub),
        )
        .route(
            "/api/users/lists/list",
            post(handlers::misskey::endpoints::users_lists_list),
        )
        .route(
            "/api/users/lists/show",
            post(handlers::misskey::endpoints::users_lists_show),
        )
        // MiAuth（Misskey 互換クライアント用）
        .route("/miauth/:session_id", get(handlers::miauth::miauth_page))
        .route(
            "/api/miauth/:session_id/authorize",
            post(handlers::miauth::miauth_authorize),
        )
        .route(
            "/api/miauth/:session_id/check",
            post(handlers::miauth::miauth_check_by_path),
        )
        .route("/api/miauth/check", post(handlers::miauth::miauth_check))
        // Misskey 旧来の app 認証フロー（app/create → auth/session/generate →
        // auth/session/userkey）。SocialHub Web 等、MiAuth 非対応クライアント向け。
        .route(
            "/api/app/create",
            post(handlers::misskey::app_auth::app_create),
        )
        .route(
            "/api/auth/session/generate",
            post(handlers::misskey::app_auth::session_generate),
        )
        .route(
            "/api/auth/session/userkey",
            post(handlers::misskey::app_auth::session_userkey),
        )
        .route("/auth/:token", get(handlers::misskey::app_auth::auth_page))
        .route(
            "/api/auth-sessions/:token",
            get(handlers::misskey::app_auth::session_info),
        )
        .route(
            "/api/auth-sessions/:token/authorize",
            post(handlers::misskey::app_auth::session_authorize),
        )
}

/// AT Protocol XRPC・DID 解決。
/// Mastodon 互換 API（`handlers::mastodon`）。エラー応答は `mastodon::error_shape` で
/// Mastodon の `{"error": "..."}` 形に揃える（`route_layer` なのでこのルーター内のルートだけ）。
fn mastodon_routes() -> Router<AppState> {
    use handlers::mastodon::{
        accounts, instance, media, notifications, oauth, search, statuses, streaming, timelines,
    };
    let stubs = [
        "/api/v1/filters",
        "/api/v2/filters",
        "/api/v1/announcements",
        "/api/v1/favourites",
        "/api/v1/conversations",
        "/api/v1/followed_tags",
        "/api/v1/featured_tags",
        "/api/v1/endorsements",
        "/api/v1/scheduled_statuses",
        "/api/v1/follow_requests",
        "/api/v1/domain_blocks",
        "/api/v1/trends",
        "/api/v1/trends/tags",
        "/api/v1/trends/statuses",
        "/api/v1/trends/links",
        "/api/v1/suggestions",
        "/api/v2/suggestions",
    ];
    let router = stubs.into_iter().fold(Router::new(), |r, path| {
        r.route(path, get(instance::empty_array))
    });
    router
        .route("/api/v1/apps", post(oauth::create_app))
        .route(
            "/api/v1/apps/verify_credentials",
            get(oauth::verify_app_credentials),
        )
        .route("/oauth/authorize", get(oauth::authorize_page))
        .route("/oauth/token", post(oauth::token))
        .route("/oauth/revoke", post(oauth::revoke))
        .route("/api/oauth/apps/:client_id", get(oauth::app_info))
        .route("/api/oauth/authorize", post(oauth::authorize))
        .route("/api/v1/instance", get(instance::instance_v1))
        .route("/api/v2/instance", get(instance::instance_v2))
        .route("/api/v1/custom_emojis", get(instance::custom_emojis))
        .route("/api/v1/preferences", get(instance::preferences))
        .route("/api/v1/markers", get(instance::empty_object))
        .route("/api/headers/missing.png", get(instance::missing_header))
        .route(
            "/api/v1/accounts/verify_credentials",
            get(accounts::verify_credentials),
        )
        .route(
            "/api/v1/accounts/relationships",
            get(accounts::relationships),
        )
        .route("/api/v1/accounts/lookup", get(accounts::lookup))
        .route("/api/v1/accounts/search", get(accounts::search))
        .route("/api/v1/accounts/:id", get(accounts::show))
        .route("/api/v1/accounts/:id/statuses", get(accounts::statuses))
        .route("/api/v1/accounts/:id/followers", get(accounts::followers))
        .route("/api/v1/accounts/:id/following", get(accounts::following))
        .route("/api/v1/accounts/:id/follow", post(accounts::follow))
        .route("/api/v1/accounts/:id/unfollow", post(accounts::unfollow))
        .route("/api/v1/accounts/:id/block", post(accounts::block))
        .route("/api/v1/accounts/:id/unblock", post(accounts::unblock))
        .route("/api/v1/accounts/:id/mute", post(accounts::mute))
        .route("/api/v1/accounts/:id/unmute", post(accounts::unmute))
        .route(
            "/api/v1/accounts/update_credentials",
            patch(accounts::update_credentials).layer(DefaultBodyLimit::max(25 * 1024 * 1024)),
        )
        .route("/api/v1/blocks", get(accounts::blocks))
        .route("/api/v1/mutes", get(accounts::mutes))
        .route("/api/v1/bookmarks", get(statuses::bookmarks))
        .route("/api/v1/statuses/:id/pin", post(statuses::pin))
        .route("/api/v1/statuses/:id/unpin", post(statuses::unpin))
        .route("/api/v1/statuses/:id/bookmark", post(statuses::bookmark))
        .route(
            "/api/v1/statuses/:id/unbookmark",
            post(statuses::unbookmark),
        )
        .route("/api/v1/polls/:id", get(statuses::poll))
        .route("/api/v1/polls/:id/votes", post(statuses::poll_vote))
        .route("/api/v1/streaming", get(streaming::streaming))
        .route("/api/v1/streaming/health", get(streaming::health))
        .route("/api/v1/timelines/home", get(timelines::home))
        .route("/api/v1/timelines/public", get(timelines::public))
        .route("/api/v1/timelines/tag/:hashtag", get(timelines::tag))
        .route("/api/v1/timelines/list/:id", get(timelines::list))
        .route("/api/v1/tags/:name", get(timelines::tag_info))
        .route("/api/v1/lists", get(timelines::lists))
        .route("/api/v1/lists/:id", get(timelines::list_show))
        .route("/api/v1/statuses", post(statuses::create))
        .route(
            "/api/v1/statuses/:id",
            get(statuses::show).delete(statuses::delete),
        )
        .route("/api/v1/statuses/:id/context", get(statuses::context))
        .route("/api/v1/statuses/:id/favourite", post(statuses::favourite))
        .route(
            "/api/v1/statuses/:id/unfavourite",
            post(statuses::unfavourite),
        )
        .route("/api/v1/statuses/:id/reblog", post(statuses::reblog))
        .route("/api/v1/statuses/:id/unreblog", post(statuses::unreblog))
        .route(
            "/api/v1/statuses/:id/reblogged_by",
            get(statuses::reblogged_by),
        )
        .route(
            "/api/v1/statuses/:id/favourited_by",
            get(statuses::favourited_by),
        )
        .route("/api/v2/search", get(search::search))
        .route(
            "/api/v1/media",
            post(media::upload).layer(DefaultBodyLimit::max(105 * 1024 * 1024)),
        )
        .route(
            "/api/v2/media",
            post(media::upload).layer(DefaultBodyLimit::max(105 * 1024 * 1024)),
        )
        .route("/api/v1/media/:id", get(media::show).put(media::show))
        .route("/api/v1/notifications", get(notifications::list))
        .route_layer(axum::middleware::from_fn(handlers::mastodon::error_shape))
}

fn xrpc_routes() -> Router<AppState> {
    Router::new()
        // AT Protocol XRPC エンドポイント
        .route(
            "/xrpc/com.atproto.server.describeServer",
            get(handlers::xrpc::server::xrpc_describe_server),
        )
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(handlers::xrpc::server::xrpc_resolve_handle),
        )
        .route(
            "/xrpc/com.atproto.sync.getRepo",
            get(handlers::xrpc::sync::xrpc_get_repo),
        )
        .route(
            "/xrpc/com.atproto.sync.getBlob",
            get(handlers::xrpc::sync::xrpc_get_blob),
        )
        .route(
            "/xrpc/com.atproto.sync.subscribeRepos",
            get(handlers::xrpc::sync::xrpc_subscribe_repos),
        )
        .route(
            "/xrpc/com.atproto.repo.getRecord",
            get(handlers::xrpc::repo::xrpc_get_record),
        )
        .route(
            "/xrpc/com.atproto.repo.listRecords",
            get(handlers::xrpc::repo::xrpc_list_records),
        )
        .route(
            "/xrpc/com.atproto.repo.describeRepo",
            get(handlers::xrpc::repo::xrpc_describe_repo),
        )
        .route(
            "/xrpc/com.atproto.repo.createRecord",
            post(handlers::xrpc::repo::xrpc_create_record),
        )
        .route(
            "/xrpc/com.atproto.repo.putRecord",
            post(handlers::xrpc::repo::xrpc_put_record),
        )
        .route(
            "/xrpc/com.atproto.repo.deleteRecord",
            post(handlers::xrpc::repo::xrpc_delete_record),
        )
        .route(
            "/xrpc/com.atproto.repo.applyWrites",
            post(handlers::xrpc::repo::xrpc_apply_writes),
        )
        .route(
            "/xrpc/com.atproto.sync.listRepos",
            get(handlers::xrpc::sync::xrpc_list_repos),
        )
        .route(
            "/xrpc/com.atproto.sync.getLatestCommit",
            get(handlers::xrpc::sync::xrpc_get_latest_commit),
        )
        .route(
            "/xrpc/com.atproto.sync.listBlobs",
            get(handlers::xrpc::sync::xrpc_list_blobs),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(handlers::xrpc::server::xrpc_create_session),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(handlers::xrpc::server::xrpc_refresh_session),
        )
        .route(
            "/xrpc/com.atproto.server.deleteSession",
            post(handlers::xrpc::server::xrpc_delete_session),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(handlers::xrpc::server::xrpc_get_session),
        )
        // 既存DID転入フロー(docs/account_migration.md)の逆方向: seiranが転出元PDSとして
        // 振る舞うためのエンドポイント群。
        .route(
            "/xrpc/com.atproto.server.checkAccountStatus",
            get(handlers::xrpc::server::xrpc_check_account_status),
        )
        .route(
            "/xrpc/com.atproto.server.deactivateAccount",
            post(handlers::xrpc::server::xrpc_deactivate_account),
        )
        .route(
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            get(handlers::xrpc::identity::xrpc_get_recommended_did_credentials),
        )
        .route(
            "/xrpc/com.atproto.identity.requestPlcOperationSignature",
            post(handlers::xrpc::identity::xrpc_request_plc_operation_signature),
        )
        .route(
            "/xrpc/com.atproto.identity.signPlcOperation",
            post(handlers::xrpc::identity::xrpc_sign_plc_operation),
        )
        .route(
            "/xrpc/com.atproto.identity.submitPlcOperation",
            post(handlers::xrpc::identity::xrpc_submit_plc_operation),
        )
        .route(
            "/xrpc/app.bsky.actor.getPreferences",
            get(handlers::xrpc::actor::xrpc_get_preferences),
        )
        .route(
            "/xrpc/app.bsky.actor.putPreferences",
            post(handlers::xrpc::actor::xrpc_put_preferences),
        )
        .route(
            "/xrpc/app.bsky.unspecced.getTrends",
            get(handlers::xrpc::actor::xrpc_get_trends),
        )
        .route(
            "/xrpc/com.atproto.server.createAppPassword",
            post(handlers::xrpc::server::xrpc_create_app_password),
        )
        .route(
            "/xrpc/com.atproto.server.listAppPasswords",
            get(handlers::xrpc::server::xrpc_list_app_passwords),
        )
        .route(
            "/xrpc/com.atproto.server.revokeAppPassword",
            post(handlers::xrpc::server::xrpc_revoke_app_password),
        )
        // Bsky公式動画パイプライン（uploadVideo）が完了後に呼び戻してくるコールバック
        .route(
            "/xrpc/com.atproto.repo.uploadBlob",
            post(handlers::xrpc::repo::xrpc_upload_blob),
        )
        // AT Protocol DID 解決
        .route(
            "/.well-known/did.json",
            get(handlers::xrpc::server::well_known_did),
        )
        .route(
            "/.well-known/atproto-did",
            get(handlers::xrpc::server::well_known_atproto_did),
        )
}

/// api ロールの axum ルーターを構築する（CORS 適用込み）。
pub fn router(state: AppState) -> Router {
    let cors = cors_layer(&state);
    Router::new()
        .merge(admin_routes(&state))
        .merge(base_routes())
        .merge(auth_account_routes())
        .merge(notes_routes())
        .merge(page_routes())
        .merge(social_routes())
        .merge(misskey_routes())
        .merge(mastodon_routes())
        .merge(xrpc_routes())
        // 未実装のXRPCメソッドへの `atproto-proxy` ヘッダー付きリクエストをAppView等へ
        // 透過転送する（明示的な `.route()` の方が優先されるため、ここに置いても既存の
        // XRPCハンドラを妨げない）。
        .fallback(handlers::xrpc::proxy::xrpc_proxy_fallback)
        .with_state(state)
        // Misskey クライアントの `i`（ボディ/クエリ）トークンを Authorization ヘッダーへ
        // 合成するブリッジ。既存ハンドラの extract_auth 呼び出しは無改修のまま両対応になる。
        .layer(axum::middleware::from_fn(
            middleware::misskey_auth_bridge::bridge,
        ))
        // フロントエンドとの互換性チェック用に、サーバーのバージョン・最低対向バージョンを
        // 全レスポンスへ付与する（docs/architecture.md 2.1節）。
        .layer(axum::middleware::from_fn(
            middleware::version_headers::attach,
        ))
        .layer(cors)
}

//! 前段に登録するパスの許可リスト（`docker/seiran/allowed-paths.txt`）の検査。
//!
//! 前段（Cloudflare Tunnel の ingress 等）はこのリストに当たらないパスを seiran へ通さない。
//! ルートを足して許可リストを直し忘れると、その URL が本番で黙って 404 になる（連合先から
//! だけ失敗する等、こちらから気づけない）。そこで次を確かめる:
//!
//! 1. バックエンドの全ルート（`crates/*/src` の `.route("...")` と Mastodon 互換のスタブ一覧）と
//!    SPA の全ルート（`frontend/src/App.tsx` の `path="..."`、管理画面の slug）から代表の URL を
//!    作り、すべて `[apex]` のどれかの行に当たること。
//! 2. 脆弱性スキャンが定番で叩くパスがどの行にも当たらないこと。
//! 3. 各行が `^` で始まり、RE2 系の文法（後方参照・先読みを含まない。Rust の `regex` が
//!    受け付けるもの）で書かれていること。
//!
//! ルート定義はソースを文字列として読む（axum の `Router` は登録済みのパスを列挙できない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("ワークスペースルートの解決に失敗")
}

/// 許可リストを節ごとに読む。
fn load_allowlist() -> BTreeMap<String, Vec<(String, Regex)>> {
    let path = workspace_root().join("docker/seiran/allowed-paths.txt");
    let text = std::fs::read_to_string(&path).expect("allowed-paths.txt を読めない");
    let mut sections: BTreeMap<String, Vec<(String, Regex)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.to_owned());
            sections.entry(name.to_owned()).or_default();
            continue;
        }
        let section = current
            .clone()
            .unwrap_or_else(|| panic!("節の外に行がある: {line}"));
        assert!(line.starts_with('^'), "`^` で始まっていない行: {line}");
        let re = Regex::new(line).unwrap_or_else(|e| panic!("正規表現として不正: {line}: {e}"));
        sections
            .entry(section)
            .or_default()
            .push((line.to_owned(), re));
    }
    sections
}

fn matches(rules: &[(String, Regex)], path: &str) -> bool {
    rules.iter().any(|(_, re)| re.is_match(path))
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `#[cfg(test)]` より前（本番のコード）だけを返す。
fn non_test_part(content: &str) -> &str {
    content
        .find("#[cfg(test)]")
        .map_or(content, |i| &content[..i])
}

/// バックエンドのルートのパス（axum の書式、`:param`・`*rest` を含む）。
fn backend_routes() -> Vec<String> {
    let root = workspace_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        collect_rs(&entry.path().join("src"), &mut files);
    }
    let route_re = Regex::new(r#"\.route\(\s*"([^"]+)""#).unwrap();
    // `let stubs = [ "/api/v1/filters", ... ];` を `r.route(path, ...)` で一括登録している
    let stubs_re = Regex::new(r"(?s)let stubs = \[(.*?)\];").unwrap();
    let literal_re = Regex::new(r#""(/[^"]*)""#).unwrap();
    let mut routes = Vec::new();
    for file in files {
        let content = std::fs::read_to_string(&file).unwrap();
        let code = non_test_part(&content);
        routes.extend(route_re.captures_iter(code).map(|c| c[1].to_owned()));
        for block in stubs_re.captures_iter(code) {
            routes.extend(literal_re.captures_iter(&block[1]).map(|c| c[1].to_owned()));
        }
    }
    routes
}

/// SPA のルートのパス（React Router の書式、`:param`・`:param?` を含む）。
fn spa_routes() -> Vec<String> {
    let app = std::fs::read_to_string(workspace_root().join("frontend/src/App.tsx")).unwrap();
    Regex::new(r#"\bpath="([^"]+)""#)
        .unwrap()
        .captures_iter(&app)
        .map(|c| c[1].to_owned())
        .collect()
}

/// 管理画面のタブの slug（`/admin/:slug?` に入る値）。
fn admin_slugs() -> Vec<String> {
    let page =
        std::fs::read_to_string(workspace_root().join("frontend/src/pages/AdminPage.tsx")).unwrap();
    Regex::new(r#"\bslug: "([^"]+)""#)
        .unwrap()
        .captures_iter(&page)
        .map(|c| c[1].to_owned())
        .collect()
}

/// パラメータに入れる代表の値。
fn sample_value(name: &str) -> &'static str {
    match name {
        "acct" => "@alice@example.com",
        "handle" => "alice",
        "slug" => "site-settings",
        n if n == "id" || n.ends_with("_id") || n.ends_with("Id") => "116916636236840960",
        _ => "alice_1",
    }
}

/// ルートのパスから代表の URL を作る。省略可能なパラメータ（`:x?`）は有無の両方を返す。
fn expand(route: &str) -> Vec<String> {
    let mut variants = vec![String::new()];
    for segment in route.split('/').skip(1) {
        let optional = segment.starts_with(':') && segment.ends_with('?');
        let segment = segment.trim_end_matches('?');
        let filled = if segment.starts_with('*') {
            "a/b.webp".to_owned()
        } else if let Some(i) = segment.find(':') {
            format!("{}{}", &segment[..i], sample_value(&segment[i + 1..]))
        } else {
            segment.to_owned()
        };
        let mut next = Vec::new();
        for v in &variants {
            if optional {
                next.push(v.clone());
            }
            next.push(format!("{v}/{filled}"));
        }
        variants = next;
    }
    variants
        .into_iter()
        .map(|v| if v.is_empty() { "/".to_owned() } else { v })
        .collect()
}

#[test]
fn allowlist_covers_every_route() {
    let allow = load_allowlist();
    let apex = &allow["apex"];
    let backend = backend_routes();
    let spa = spa_routes();
    // 抽出が壊れて0件で素通りしないように
    assert!(
        backend.len() > 200,
        "バックエンドのルートを抽出できていない: {}",
        backend.len()
    );
    assert!(
        spa.len() > 20,
        "SPA のルートを抽出できていない: {}",
        spa.len()
    );
    let slugs = admin_slugs();
    assert!(!slugs.is_empty(), "管理画面の slug を抽出できていない");

    let mut urls: Vec<(String, String)> = Vec::new();
    for r in backend.iter().chain(&spa) {
        urls.extend(expand(r).into_iter().map(|u| (u, r.clone())));
    }
    for s in &slugs {
        urls.push((format!("/admin/{s}"), "AdminPage.tsx の slug".to_owned()));
    }
    // ルート定義に現れない配信物と、他のサーバー・クライアントが取りに来る URL
    for u in [
        "/assets/index-osZaFuLo.css",
        "/assets/Panel.module-OWV4tJ4o.js",
        "/twemoji/1f004.svg",
        "/twemoji/1f3f3-fe0f-200d-1f308.svg",
        "/tags/日本語",
        "/manifest.json",
        "/robots.txt",
        "/url",
        "/emojis/blobcat",
        "/emoji/blobcat.webp",
        "/avatar/@alice@example.com",
    ] {
        urls.push((u.to_owned(), "固定の配信物".to_owned()));
    }

    let missing: Vec<String> = urls
        .iter()
        .filter(|(u, _)| !matches(apex, u))
        .map(|(u, r)| format!("{u}（{r}）"))
        .collect();
    assert!(
        missing.is_empty(),
        "docker/seiran/allowed-paths.txt の [apex] に当たらない URL があります。\
         ルートを足したら許可リストにも足してください:\n{}",
        missing.join("\n")
    );

    let subdomains = &allow["subdomains"];
    assert!(matches(subdomains, "/.well-known/atproto-did"));
}

#[test]
fn allowlist_rejects_scanner_paths() {
    let allow = load_allowlist();
    let all: Vec<(String, Regex)> = allow.values().flatten().cloned().collect();
    for path in [
        "/wp-login.php",
        "/wp-admin/install.php",
        "/wp-content/plugins/x/cache.php",
        "/xmlrpc.php",
        "/.env",
        "/.env.production",
        "/.git/config",
        "/admin/.env",
        "/admin/phpinfo.php",
        "/settings/.env",
        "/messages/.env",
        "/assets/.env",
        "/assets/index.php",
        "/src/.env",
        "/vendor/phpunit/phpunit/src/Util/PHP/eval-stdin.php",
        "/cgi-bin/luci",
        "/images/cache.php",
        "/users/alice/../../etc/passwd",
        "/phpinfo.php",
        "/.well-known/security.txt",
    ] {
        assert!(
            !matches(&all, path),
            "スキャンのパスが許可リストに当たる: {path}"
        );
    }
}

#[test]
fn expand_fills_parameters() {
    assert_eq!(expand("/"), vec!["/"]);
    assert_eq!(expand("/@:handle"), vec!["/@alice"]);
    assert_eq!(expand("/proxy/*rest"), vec!["/proxy/a/b.webp"]);
    assert_eq!(
        expand("/admin/:slug?"),
        vec!["/admin", "/admin/site-settings"]
    );
    assert_eq!(
        expand("/messages/:threadRootId"),
        vec!["/messages/116916636236840960"]
    );
}

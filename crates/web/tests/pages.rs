mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::StoredInstallation;
use support::*;

const ALICE: u64 = 1_000_000_000_000_000_001;
const REPO: &str = "alice/bot";

struct Scene {
    app: App,
    alice: (String, String),
}

// Alice a un bot (1) avec ses deux jetons Discord et un dépôt GitHub visible.
async fn scene() -> Scene {
    let app = App::with_limits(true, false, 10, 0);
    let alice = app.dev_login("alice").await;
    app.post(
        "/products",
        Some(&alice.0),
        &format!("name=Alice+Bot&csrf={}", alice.1),
    )
    .await;
    app.seed_bot(1, "free", "free.tok", "111");
    app.seed_bot(1, "premium", "premium.tok", "222");
    app.github
        .offer(7, REPO, &[("main", HEAD_A), ("dev", HEAD_B)]);
    let stored = app.store.lock().is_ok_and(|s| {
        s.replace_installations(
            ALICE,
            &[StoredInstallation {
                id: 7,
                login: "alice".to_string(),
                selection: "selected".to_string(),
            }],
        )
        .is_ok()
    });
    assert!(stored);
    Scene { app, alice }
}

async fn connect(s: &Scene, tier: &str, extra: &str) {
    let form = format!(
        "csrf={}&installation=7&repo={}&branch=main{extra}",
        s.alice.1,
        form_encode(REPO)
    );
    let r = s
        .app
        .post(
            &format!("/products/1/source/{tier}"),
            Some(&s.alice.0),
            &form,
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
}

fn product_exists(s: &Scene) -> bool {
    s.app
        .store
        .lock()
        .is_ok_and(|st| matches!(st.product(ProductId(1)), Ok(Some(_))))
}

#[tokio::test]
async fn account_pages_have_the_app_header_with_scope_tabs_and_search() {
    let s = scene().await;
    for path in ["/dashboard", "/servers", "/github", "/new"] {
        let page = s.app.get(path, Some(&s.alice.0)).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(
            page.body
                .contains(r#"<nav class="tabs" aria-label="Account">"#),
            "{path}"
        );
        assert!(
            page.body.contains(r#"class="scope" href="/dashboard""#),
            "{path}"
        );
        assert!(page.body.contains(r#"data-action="palette""#), "{path}");
        assert!(page.body.contains(r#"<dialog id="palette""#), "{path}");
        assert!(!page.body.contains(r#"<nav class="nav""#), "{path}");
    }
    let dashboard = s.app.get("/dashboard", Some(&s.alice.0)).await.body;
    assert!(
        dashboard.contains(r#"href="/dashboard" aria-current="page">Bots<span class="n">1</span>"#)
    );
    assert!(dashboard.contains(r#"href="/servers">Servers<span class="n">0</span>"#));
}

#[tokio::test]
async fn the_command_menu_lists_my_bots_and_the_docs() {
    let s = scene().await;
    let page = s.app.get("/dashboard", Some(&s.alice.0)).await.body;
    assert!(page.contains(r#"<div class="palette-group">Your bots</div>"#));
    assert!(page.contains(r#"href="/products/1" data-q="alice bot""#));
    assert!(page.contains(r#"href="/docs/bot-toml""#));
    let visitor = s.app.get("/docs", None).await.body;
    assert!(!visitor.contains("Your bots</div>"));
    assert!(visitor.contains(r#"href="/login" data-q="log in""#));
}

#[tokio::test]
async fn public_pages_keep_the_site_navigation() {
    let s = scene().await;
    let docs = s.app.get("/docs", Some(&s.alice.0)).await.body;
    assert!(docs.contains(r#"<nav class="nav" aria-label="Main">"#));
    assert!(!docs.contains(r#"aria-label="Account""#));
    let anonymous = s.app.get("/", None).await.body;
    assert!(anonymous.contains(r#"href="/login""#) && anonymous.contains("Marketplace"));
}

#[tokio::test]
async fn a_bot_has_five_tabs_and_its_name_in_the_header() {
    let s = scene().await;
    let page = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(page.contains(r#"<nav class="tabs" aria-label="Bot">"#));
    for tab in [
        r#"href="/products/1" aria-current="page">Overview"#,
        r#"href="/products/1/deployments">Deployments<span class="n">0</span>"#,
        r#"href="/products/1/source">Source"#,
        r#"href="/products/1/bot">Discord"#,
        r#"href="/products/1/settings">Settings"#,
    ] {
        assert!(page.contains(tab), "{tab}");
    }
    assert!(page.contains(r#"class="scope" href="/products/1""#));
}

#[tokio::test]
async fn the_new_bot_page_needs_a_login_and_posts_to_the_existing_route() {
    let s = scene().await;
    let anonymous = s.app.get("/new", None).await;
    assert_eq!(anonymous.location(), Some("/login"));
    let page = s.app.get("/new", Some(&s.alice.0)).await.body;
    assert!(page.contains(r#"method="post" action="/products""#));
    assert!(page.contains(&format!(r#"name="csrf" value="{}""#, s.alice.1)));
    assert!(page.contains(r#"name="name""#) && page.contains("Repositories you can import"));
}

#[tokio::test]
async fn the_servers_page_is_empty_without_subscriptions() {
    let s = scene().await;
    let page = s.app.get("/servers", Some(&s.alice.0)).await.body;
    assert!(page.contains("No subscriptions") && page.contains(r#"href="/market""#));
    assert!(s.app.get("/servers", None).await.location() == Some("/login"));
}

#[tokio::test]
async fn the_deployments_tab_filters_by_version_status_and_branch() {
    let s = scene().await;
    connect(&s, "free", "").await;
    connect(&s, "premium", "").await;
    for (tier, branch) in [("free", "main"), ("premium", "dev")] {
        let r = s
            .app
            .post(
                "/products/1/deployments",
                Some(&s.alice.0),
                &format!("csrf={}&tier={tier}&branch={branch}", s.alice.1),
            )
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
    }
    let all = s
        .app
        .get("/products/1/deployments", Some(&s.alice.0))
        .await
        .body;
    assert_eq!(all.matches(r#"<div class="dep">"#).count(), 2);
    let free = s
        .app
        .get("/products/1/deployments?tier=free", Some(&s.alice.0))
        .await
        .body;
    assert_eq!(free.matches(r#"<div class="dep">"#).count(), 1);
    assert!(free.contains(r#"name="tier" value="free" aria-pressed="true""#));
    let dev = s
        .app
        .get("/products/1/deployments?branch=dev", Some(&s.alice.0))
        .await
        .body;
    assert_eq!(dev.matches(r#"<div class="dep">"#).count(), 1);
    let failed = s
        .app
        .get("/products/1/deployments?status=failed", Some(&s.alice.0))
        .await
        .body;
    assert!(failed.contains("No deployment matches these filters"));
    let nobody = s.app.get("/products/1/deployments", None).await;
    assert_eq!(nobody.location(), Some("/login"));
}

#[tokio::test]
async fn only_the_owner_sees_the_bot_tabs() {
    let s = scene().await;
    let (bob, _) = s.app.dev_login("bob").await;
    for path in [
        "/products/1/deployments",
        "/products/1/source",
        "/products/1/bot",
        "/products/1/settings",
    ] {
        assert_eq!(
            s.app.get(path, Some(&bob)).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(
            s.app.get(path, None).await.location(),
            Some("/login"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn the_source_tab_shows_each_version_and_lets_auto_deploys_be_switched() {
    let s = scene().await;
    let bare = s.app.get("/products/1/source", Some(&s.alice.0)).await.body;
    assert_eq!(bare.matches("No repository is connected").count(), 2);
    connect(&s, "free", "&auto_deploy=1").await;
    let linked = s.app.get("/products/1/source", Some(&s.alice.0)).await.body;
    assert!(linked.contains("alice/bot") && linked.contains("Disconnect"));
    assert!(linked.contains(r#"name="auto_deploy" value="1" data-autosubmit checked"#));

    let off = s
        .app
        .post(
            "/products/1/source/free/auto",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(off.location(), Some("/products/1/source"));
    let stored = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten());
    assert_eq!(stored.map(|x| x.auto_deploy), Some(false));

    s.app.github.push(REPO, "main", HEAD_B);
    let on = s
        .app
        .post(
            "/products/1/source/free/auto",
            Some(&s.alice.0),
            &format!("csrf={}&auto_deploy=1", s.alice.1),
        )
        .await;
    assert_eq!(on.location(), Some("/products/1/source"));
    let stored = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten());
    // Activer ne déploie rien : la tête actuelle est notée, seul le prochain push comptera.
    assert_eq!(
        stored.map(|x| (x.auto_deploy, x.last_head)),
        Some((true, HEAD_B.to_string()))
    );
}

#[tokio::test]
async fn the_auto_deploy_switch_checks_the_form_token_the_owner_and_the_version() {
    let s = scene().await;
    connect(&s, "free", "").await;
    let wrong = s
        .app
        .post(
            "/products/1/source/free/auto",
            Some(&s.alice.0),
            "csrf=wrong",
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let (bob, bob_csrf) = s.app.dev_login("bob").await;
    let foreign = s
        .app
        .post(
            "/products/1/source/free/auto",
            Some(&bob),
            &format!("csrf={bob_csrf}"),
        )
        .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    let unknown = s
        .app
        .post(
            "/products/1/source/gold/auto",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let unconnected = s
        .app
        .post(
            "/products/1/source/premium/auto",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(unconnected.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_discord_tab_says_which_versions_have_a_verified_token() {
    let app = App::with_limits(true, false, 10, 0);
    let (alice, csrf) = app.dev_login("alice").await;
    app.post(
        "/products",
        Some(&alice),
        &format!("name=Alice+Bot&csrf={csrf}"),
    )
    .await;
    let bare = app.get("/products/1/bot", Some(&alice)).await.body;
    assert_eq!(
        bare.matches(r#"<span class="chip">No token</span>"#)
            .count(),
        2
    );
    app.seed_bot(1, "free", "free.tok", "111");
    let page = app.get("/products/1/bot", Some(&alice)).await.body;
    assert_eq!(
        page.matches(r#"<span class="chip chip-ok">Token verified</span>"#)
            .count(),
        1
    );
    assert_eq!(
        page.matches(r#"<span class="chip">No token</span>"#)
            .count(),
        1
    );
    assert!(page.contains("client_id=111") && !page.contains("free.tok"));
}

#[tokio::test]
async fn renaming_a_bot_checks_the_name_the_token_and_the_owner() {
    let s = scene().await;
    let ok = s
        .app
        .post(
            "/products/1/settings",
            Some(&s.alice.0),
            &format!("csrf={}&name=Renamed+Bot", s.alice.1),
        )
        .await;
    assert_eq!(ok.location(), Some("/products/1/settings"));
    assert!(s
        .app
        .get("/products/1", Some(&s.alice.0))
        .await
        .body
        .contains("Renamed Bot"));
    for bad in ["ab", "<script>"] {
        let r = s
            .app
            .post(
                "/products/1/settings",
                Some(&s.alice.0),
                &format!("csrf={}&name={}", s.alice.1, form_encode(bad)),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let forged = s
        .app
        .post(
            "/products/1/settings",
            Some(&s.alice.0),
            "csrf=x&name=Hacked+Bot",
        )
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let (bob, bob_csrf) = s.app.dev_login("bob").await;
    let foreign = s
        .app
        .post(
            "/products/1/settings",
            Some(&bob),
            &format!("csrf={bob_csrf}&name=Stolen+Bot"),
        )
        .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    assert!(s
        .app
        .get("/products/1", Some(&s.alice.0))
        .await
        .body
        .contains("Renamed Bot"));
}

#[tokio::test]
async fn stopping_both_versions_stops_both_containers() {
    let s = scene().await;
    let r = s
        .app
        .post(
            "/products/1/stop-all",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1"));
    let stopped = s
        .app
        .containers
        .stopped
        .lock()
        .map(|l| l.clone())
        .unwrap_or_default();
    assert_eq!(stopped, ["bot-1-free", "bot-1-premium"]);
    let forged = s
        .app
        .post("/products/1/stop-all", Some(&s.alice.0), "csrf=x")
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deleting_a_bot_needs_its_exact_name_the_token_and_the_owner() {
    let s = scene().await;
    let (bob, bob_csrf) = s.app.dev_login("bob").await;
    let foreign = s
        .app
        .post(
            "/products/1/delete",
            Some(&bob),
            &format!("csrf={bob_csrf}&confirm=Alice+Bot"),
        )
        .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    let forged = s
        .app
        .post(
            "/products/1/delete",
            Some(&s.alice.0),
            "csrf=x&confirm=Alice+Bot",
        )
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let wrong = s
        .app
        .post(
            "/products/1/delete",
            Some(&s.alice.0),
            &format!("csrf={}&confirm=alice+bot", s.alice.1),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert!(product_exists(&s));
    assert!(s
        .app
        .containers
        .stopped
        .lock()
        .map(|l| l.is_empty())
        .unwrap_or(false));

    let done = s
        .app
        .post(
            "/products/1/delete",
            Some(&s.alice.0),
            &format!("csrf={}&confirm=Alice+Bot", s.alice.1),
        )
        .await;
    assert_eq!(done.location(), Some("/dashboard"));
    assert!(!product_exists(&s));
    assert!(s.app.stored_credential(1, "free").is_none());
    assert_eq!(
        s.app.get("/products/1", Some(&s.alice.0)).await.status,
        StatusCode::NOT_FOUND
    );
    let stopped = s
        .app
        .containers
        .stopped
        .lock()
        .map(|l| l.clone())
        .unwrap_or_default();
    assert_eq!(stopped, ["bot-1-free", "bot-1-premium"]);
}

#[tokio::test]
async fn the_settings_page_has_the_three_cards() {
    let s = scene().await;
    let page = s
        .app
        .get("/products/1/settings", Some(&s.alice.0))
        .await
        .body;
    assert!(page.contains(r#"value="Alice Bot""#));
    assert!(page.contains(r#"action="/products/1/stop-all""#));
    assert!(page.contains(r#"action="/products/1/delete""#));
    assert!(page.contains("Type <strong>Alice Bot</strong> to confirm"));
}

#[tokio::test]
async fn a_bot_listing_hides_unlisted_bots_from_everyone_but_their_owner() {
    let app = App::with_limits(true, false, 10, 0);
    let (alice, csrf) = app.dev_login("alice").await;
    let (bob, _) = app.dev_login("bob").await;
    app.post(
        "/products",
        Some(&alice),
        &format!("name=Hidden+Bot&csrf={csrf}"),
    )
    .await;
    assert_eq!(
        app.get("/bots/1", Some(&bob)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/bots/1", Some(&alice)).await.status,
        StatusCode::OK
    );
    app.seed_bot(1, "free", "t", "111");
    let page = app.get("/bots/1", Some(&bob)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Hidden Bot") && page.body.contains("By alice (dev)"));
    assert_eq!(
        app.get("/bots/99", Some(&bob)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.get("/bots/1", None).await.location(), Some("/login"));
}

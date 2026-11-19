mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::Plan;
use support::*;

const DAY: i64 = 86_400;

#[tokio::test]
async fn visitors_get_a_landing_page_with_security_headers() {
    let app = App::new(false, true);
    let r = app.get("/", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Log in"));
    let header = |name: &str| {
        r.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    assert!(header("content-security-policy").contains("default-src 'none'"));
    assert!(header("content-security-policy").contains("frame-ancestors 'none'"));
    assert_eq!(header("x-frame-options"), "DENY");
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(header("referrer-policy"), "no-referrer");
    assert_eq!(header("cache-control"), "no-store");
}

#[tokio::test]
async fn the_stylesheet_is_served_from_our_own_origin() {
    let app = App::new(false, true);
    let r = app.get("/static/app.css", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let kind = r
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(kind.starts_with("text/css"));
    assert!(r.body.contains("--blue"));
}

#[tokio::test]
async fn private_pages_send_visitors_to_the_login_page() {
    let app = App::new(true, false);
    for path in ["/dashboard", "/market", "/products/1"] {
        let r = app.get(path, None).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(r.location(), Some("/login"), "{path}");
    }
    for (path, form) in [
        ("/products", "name=abc&csrf=x"),
        ("/subscribe", "product=1&guild=1&csrf=x"),
        ("/subscriptions/cancel", "product=1&guild=1&csrf=x"),
        ("/logout", "csrf=x"),
        ("/products/1/deploy", "tier=free&csrf=x"),
        ("/products/1/stop", "tier=free&csrf=x"),
    ] {
        let r = app.post(path, None, form).await;
        assert_eq!(r.location(), Some("/login"), "{path}");
    }
}

#[tokio::test]
async fn a_forged_cookie_is_not_a_session() {
    let app = App::new(true, false);
    let r = app
        .get("/dashboard", Some("session=0123456789abcdef"))
        .await;
    assert_eq!(r.location(), Some("/login"));
}

#[tokio::test]
async fn the_login_page_offers_what_is_configured() {
    let dev_only = App::new(true, false).get("/login", None).await;
    assert_eq!(dev_only.status, StatusCode::OK);
    assert!(dev_only.body.contains("Log in as alice") && dev_only.body.contains("Log in as bob"));
    assert!(!dev_only.body.contains("Log in with Discord"));

    let both = App::new(true, true).get("/login", None).await;
    assert!(both.body.contains("Log in with Discord"));

    let discord_only = App::new(false, true).get("/login", None).await;
    assert_eq!(discord_only.status, StatusCode::SEE_OTHER);
    assert_eq!(discord_only.location(), Some("/auth/discord"));

    let nothing = App::new(false, false).get("/login", None).await;
    assert_eq!(nothing.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn dev_login_is_off_unless_enabled() {
    let app = App::new(false, true);
    let r = app.post("/dev-login", None, "who=alice").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.cookie("session").is_none());
}

#[tokio::test]
async fn dev_login_creates_a_hardened_session() {
    let app = App::new(true, false);
    let r = app.post("/dev-login", None, "who=alice").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), Some("/dashboard"));
    let cookie = r
        .set_cookies()
        .into_iter()
        .find(|c| c.starts_with("session="))
        .unwrap_or_default();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Lax")
            && cookie.contains("Max-Age=604800")
    );
    assert!(!cookie.contains("Secure"));
    let (cookie, _) = app.dev_login("alice").await;
    let dashboard = app.get("/dashboard", Some(&cookie)).await;
    assert_eq!(dashboard.status, StatusCode::OK);
    assert!(dashboard.body.contains("alice (dev)"));
}

#[tokio::test]
async fn an_unknown_dev_user_is_refused() {
    let app = App::new(true, false);
    let r = app.post("/dev-login", None, "who=mallory").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.cookie("session").is_none());
}

#[tokio::test]
async fn every_login_gets_a_new_session_id() {
    let app = App::new(true, false);
    let (a, _) = app.dev_login("alice").await;
    let (b, _) = app.dev_login("alice").await;
    assert_ne!(a, b);
}

#[tokio::test]
async fn sessions_expire_after_seven_days() {
    let app = App::new(true, false);
    let (cookie, _) = app.dev_login("alice").await;
    app.advance(7 * DAY - 1);
    assert_eq!(
        app.get("/dashboard", Some(&cookie)).await.status,
        StatusCode::OK
    );
    app.advance(1);
    assert_eq!(
        app.get("/dashboard", Some(&cookie)).await.location(),
        Some("/login")
    );
}

#[tokio::test]
async fn the_discord_redirect_carries_a_state_that_matches_its_cookie() {
    let app = App::new(false, true);
    let r = app.get("/auth/discord", None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let state = r.cookie("oauth_state").unwrap_or_default();
    assert_eq!(state.len(), 64);
    assert_eq!(
        r.location(),
        Some(format!("https://discord.test/authorize?state={state}").as_str())
    );
    let cookie = r
        .set_cookies()
        .into_iter()
        .find(|c| c.starts_with("oauth_state="))
        .unwrap_or_default();
    assert!(cookie.contains("HttpOnly") && cookie.contains("Max-Age=600"));
    assert_eq!(
        App::new(false, false)
            .get("/auth/discord", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_callback_refuses_incomplete_or_forged_requests() {
    let app = App::new(false, true);
    let with_state = Some("oauth_state=abc");
    for (path, cookie) in [
        ("/auth/callback", None),
        ("/auth/callback?code=good", with_state),
        ("/auth/callback?state=abc", with_state),
        ("/auth/callback?code=good&state=abc", None),
        ("/auth/callback?code=good&state=other", with_state),
        ("/auth/callback?error=access_denied&state=abc", with_state),
    ] {
        let r = app.get(path, cookie).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}");
        assert!(r.cookie("session").is_none(), "{path}");
    }
}

#[tokio::test]
async fn a_rejected_code_is_a_bad_gateway_without_a_session() {
    let app = App::new(false, true);
    let r = app
        .get("/auth/callback?code=bad&state=abc", Some("oauth_state=abc"))
        .await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.cookie("session").is_none());
}

#[tokio::test]
async fn a_valid_callback_logs_the_user_in_and_clears_the_state() {
    let app = App::new(false, true);
    let r = app
        .get(
            "/auth/callback?code=good&state=abc",
            Some("oauth_state=abc"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), Some("/dashboard"));
    let session = r.cookie("session").unwrap_or_default();
    let cleared = r
        .set_cookies()
        .into_iter()
        .find(|c| c.starts_with("oauth_state="))
        .unwrap_or_default();
    assert!(cleared.contains("Max-Age=0"));
    app.accept_cgu_for(&session);
    let dashboard = app
        .get("/dashboard", Some(&format!("session={session}")))
        .await;
    assert!(dashboard.body.contains("dana"));
}

#[tokio::test]
async fn logging_out_needs_the_form_token_and_ends_the_session() {
    let app = App::new(true, false);
    let (cookie, csrf) = app.dev_login("alice").await;
    assert!(app
        .post("/logout", Some(&cookie), "")
        .await
        .status
        .is_client_error());
    assert_eq!(
        app.post("/logout", Some(&cookie), "csrf=wrong")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get("/dashboard", Some(&cookie)).await.status,
        StatusCode::OK
    );
    let r = app
        .post("/logout", Some(&cookie), &format!("csrf={csrf}"))
        .await;
    assert_eq!(r.location(), Some("/"));
    let cleared = r
        .set_cookies()
        .into_iter()
        .find(|c| c.starts_with("session="))
        .unwrap_or_default();
    assert!(cleared.contains("Max-Age=0"));
    assert_eq!(
        app.get("/dashboard", Some(&cookie)).await.location(),
        Some("/login")
    );
}

#[tokio::test]
async fn guild_names_from_discord_cannot_inject_html() {
    let app = App::new(false, true);
    if let Ok(mut login) = app.discord.login.lock() {
        *login = Ok((
            user(78, "<b>evil</b>"),
            vec![guild("600", "<img src=x onerror=alert(1)>")],
        ));
    }
    let r = app
        .get("/auth/callback?code=good&state=s", Some("oauth_state=s"))
        .await;
    let session_val = r.cookie("session").unwrap_or_default();
    app.accept_cgu_for(&session_val);
    let cookie = format!("session={session_val}");
    let csrf = app.csrf_of(&session_val);
    let create = format!("name={}&csrf={csrf}", form_encode("Some Bot"));
    assert_eq!(
        app.post("/products", Some(&cookie), &create).await.status,
        StatusCode::SEE_OTHER
    );
    app.seed_bot(1, "premium", "t", "222");
    assert!(app.store.lock().is_ok_and(|store| store
        .replace_plans(
            ProductId(1),
            &[Plan {
                id: 0,
                name: "Pro".to_string(),
                price_cents: 500,
                days: 30
            }]
        )
        .is_ok()));
    for path in ["/market", "/dashboard"] {
        let page = app.get(path, Some(&cookie)).await;
        assert!(!page.body.contains("<img src=x"), "{path}");
        assert!(!page.body.contains("<b>evil"), "{path}");
        assert!(page.body.contains("&lt;b&gt;evil&lt;/b&gt;"), "{path}");
    }
    assert!(app
        .get("/bots/1", Some(&cookie))
        .await
        .body
        .contains("&lt;img src=x onerror=alert(1)&gt;"));
}

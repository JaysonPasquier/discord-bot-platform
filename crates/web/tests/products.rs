mod support;

use axum::http::StatusCode;
use platform_runner::{ContainerState, Tier};
use support::*;

async fn create(app: &App, cookie: &str, csrf: &str, name: &str) -> Resp {
    app.post(
        "/products",
        Some(cookie),
        &format!("name={}&csrf={csrf}", form_encode(name)),
    )
    .await
}

fn deployed(app: &App) -> Vec<platform_runner::RunSpec> {
    app.containers
        .deployed
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default()
}

fn env(spec: &platform_runner::RunSpec, name: &str) -> Option<String> {
    spec.env
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

#[tokio::test]
async fn creating_a_bot_needs_the_form_token() {
    let app = App::new(true, false);
    let (cookie, csrf) = app.dev_login("alice").await;
    assert!(app
        .post("/products", Some(&cookie), "name=Some+Bot")
        .await
        .status
        .is_client_error());
    let wrong = create(&app, &cookie, "wrong", "Some Bot").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let ok = create(&app, &cookie, &csrf, "Some Bot").await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location(), Some("/products/1"));
}

#[tokio::test]
async fn each_user_can_create_a_limited_number_of_bots() {
    let app = App::with_limits(true, false, 2, 0);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, b_csrf) = app.dev_login("bob").await;
    assert_eq!(
        create(&app, &alice, &a_csrf, "Bot one").await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        create(&app, &alice, &a_csrf, "Bot two").await.status,
        StatusCode::SEE_OTHER
    );
    let third = create(&app, &alice, &a_csrf, "Bot three").await;
    assert_eq!(third.status, StatusCode::FORBIDDEN);
    assert!(third.body.contains("at most 2 bots"));
    assert!(!app
        .get("/dashboard", Some(&alice))
        .await
        .body
        .contains("Bot three"));
    assert_eq!(
        create(&app, &bob, &b_csrf, "Bobs bot").await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn the_limit_is_exactly_the_configured_count() {
    let app = App::with_limits(true, false, 1, 0);
    let (alice, csrf) = app.dev_login("alice").await;
    assert_eq!(
        create(&app, &alice, &csrf, "Bot one").await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        create(&app, &alice, &csrf, "Bot two").await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_container_cannot_be_redeployed_during_the_cooldown() {
    let app = App::with_limits(true, false, 10, 30);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    let deploy = |tier: &'static str| {
        let (app, alice, csrf) = (&app, alice.clone(), csrf.clone());
        async move {
            app.post(
                "/products/1/deploy",
                Some(&alice),
                &format!("tier={tier}&csrf={csrf}"),
            )
            .await
        }
    };
    assert_eq!(deploy("free").await.status, StatusCode::OK);
    let first_token = env(&deployed(&app)[0], "PLATFORM_API_TOKEN").unwrap_or_default();

    let again = deploy("free").await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(again.body.contains("Wait 30 more second(s)"));
    assert!(again.body.contains(r#"<a href="/products/1">Back</a>"#));
    assert_eq!(deployed(&app).len(), 1);
    assert_eq!(app.product_for_token(&first_token).map(|p| p.0), Some(1));

    assert_eq!(deploy("premium").await.status, StatusCode::OK);
    app.advance(29);
    assert_eq!(deploy("free").await.status, StatusCode::TOO_MANY_REQUESTS);
    app.advance(1);
    assert_eq!(deploy("free").await.status, StatusCode::OK);
    assert_eq!(deployed(&app).len(), 3);
    assert_eq!(app.product_for_token(&first_token), None);
}

#[tokio::test]
async fn bot_names_follow_the_configuration_rule() {
    let app = App::new(true, false);
    let (cookie, csrf) = app.dev_login("alice").await;
    for (name, ok) in [
        ("ab", false),
        ("abc", true),
        (&"a".repeat(32), true),
        (&"a".repeat(33), false),
        ("<b>x</b>", false),
        (" lead", false),
        ("it's", false),
    ] {
        let r = create(&app, &cookie, &csrf, name).await;
        let expected = if ok {
            StatusCode::SEE_OTHER
        } else {
            StatusCode::BAD_REQUEST
        };
        assert_eq!(r.status, expected, "{name:?}");
    }
}

#[tokio::test]
async fn the_dashboard_lists_my_bots_only() {
    let app = App::new(true, false);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, b_csrf) = app.dev_login("bob").await;
    create(&app, &alice, &a_csrf, "Alice Bot").await;
    create(&app, &bob, &b_csrf, "Bob Bot").await;
    let page = app.get("/dashboard", Some(&alice)).await;
    assert!(page.body.contains("Alice Bot") && page.body.contains("/products/1"));
    assert!(!page.body.contains("Bob Bot"));
}

#[tokio::test]
async fn only_the_owner_can_open_a_bot_page() {
    let app = App::new(true, false);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, _) = app.dev_login("bob").await;
    create(&app, &alice, &a_csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    let owner = app.get("/products/1", Some(&alice)).await;
    assert_eq!(owner.status, StatusCode::OK);
    assert!(
        owner.body.contains("Alice Bot")
            && owner.body.contains("Free version")
            && owner.body.contains("Premium version")
    );
    assert!(owner.body.contains("Bot 111") && owner.body.contains("Bot 222"));
    let discord = app.get("/products/1/bot", Some(&alice)).await.body;
    assert!(discord.contains("client_id=111") && discord.contains("client_id=222"));
    assert_eq!(
        app.get("/products/1", Some(&bob)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/products/99", Some(&alice)).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn each_version_card_controls_its_own_tier() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    let body = app.get("/products/1", Some(&alice)).await.body;
    let at = |needle: &str| body.find(needle).unwrap_or(usize::MAX);
    let (free, free_form) = (at("Free version"), at(r#"name="tier" value="free""#));
    let (premium, premium_form) = (at("Premium version"), at(r#"name="tier" value="premium""#));
    assert!(
        free < free_form && free_form < premium && premium < premium_form,
        "{body}"
    );
}

#[tokio::test]
async fn the_bot_page_shows_container_states_and_escapes_logs() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    assert!(app
        .get("/products/1", Some(&alice))
        .await
        .body
        .contains("No container"));
    if let Ok(mut states) = app.containers.states.lock() {
        states.insert("bot-1-free".to_string(), ContainerState::Running);
        states.insert("bot-1-premium".to_string(), ContainerState::Exited(3));
    }
    if let Ok(mut logs) = app.containers.logs.lock() {
        *logs = "<script>alert(1)</script>\nhello".to_string();
    }
    let page = app.get("/products/1", Some(&alice)).await;
    assert!(page.body.contains("Container</dt><dd>Running"));
    assert!(page.body.contains("Stopped (exit code 3)"));
    assert!(page.body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!page.body.contains("<script>alert"));
}

#[tokio::test]
async fn deploying_registers_a_fresh_token_and_injects_the_secrets() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    let r = app
        .post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=free&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let specs = deployed(&app);
    assert_eq!(specs.len(), 1);
    let spec = &specs[0];
    assert_eq!(
        (
            spec.name.as_str(),
            spec.tier,
            spec.image.as_str(),
            spec.network.as_str()
        ),
        ("bot-1-free", Tier::Free, "bot:test", "bridge")
    );
    assert_eq!(
        env(spec, "PLATFORM_API_URL").as_deref(),
        Some("http://host.docker.internal:8080")
    );
    assert_eq!(
        env(spec, "DISCORD_TOKEN").as_deref(),
        Some("free-discord-secret")
    );
    assert_eq!(env(spec, "TIER").as_deref(), Some("free"));
    assert_eq!(
        env(spec, "DASHBOARD_URL").as_deref(),
        Some("http://127.0.0.1:8080")
    );
    let token = env(spec, "PLATFORM_API_TOKEN").unwrap_or_default();
    assert_eq!(token.len(), 64);
    assert_eq!(app.product_for_token(&token).map(|p| p.0), Some(1));
    assert!(!r.body.contains(&token) && !r.body.contains("free-discord-secret"));
    assert!(r.body.contains(r#"<a href="/products/1">Back</a>"#));
    assert!(!app
        .get("/products/1", Some(&alice))
        .await
        .body
        .contains(&token));
}

#[tokio::test]
async fn deploying_again_replaces_the_token_of_that_container_only() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    let post = |tier: &'static str| {
        let (app, alice, csrf) = (&app, alice.clone(), csrf.clone());
        async move {
            app.post(
                "/products/1/deploy",
                Some(&alice),
                &format!("tier={tier}&csrf={csrf}"),
            )
            .await
        }
    };
    post("free").await;
    post("premium").await;
    post("free").await;
    let specs = deployed(&app);
    let token = |i: usize| env(&specs[i], "PLATFORM_API_TOKEN").unwrap_or_default();
    assert_ne!(token(0), token(2));
    assert_eq!(app.product_for_token(&token(0)), None);
    assert_eq!(app.product_for_token(&token(2)).map(|p| p.0), Some(1));
    assert_eq!(app.product_for_token(&token(1)).map(|p| p.0), Some(1));
}

#[tokio::test]
async fn deploying_without_a_connected_bot_is_refused_and_costs_nothing() {
    let app = App::with_limits(true, false, 10, 30);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    let deploy = || async {
        app.post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=premium&csrf={csrf}"),
        )
        .await
    };
    let refused = deploy().await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.body.contains("Connect this version"));
    assert!(deployed(&app).is_empty());
    app.seed_bot(1, "premium", "premium-discord-secret", "222");
    assert_eq!(deploy().await.status, StatusCode::OK);
    let specs = deployed(&app);
    assert_eq!(specs[0].name, "bot-1-premium");
    assert_eq!(
        env(&specs[0], "DISCORD_TOKEN").as_deref(),
        Some("premium-discord-secret")
    );
    assert_eq!(env(&specs[0], "TIER").as_deref(), Some("premium"));
}

#[tokio::test]
async fn a_stored_token_that_cannot_be_decrypted_asks_for_a_new_one() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "tok", "111");
    let other = platform_store::Vault::new(&[1; 32]);
    let Ok(sealed) = other.seal("tok", "bot-credential:1:free") else {
        panic!("cannot seal");
    };
    let saved = app.store.lock().map(|s| {
        s.save_credential(
            platform_core::ProductId(1),
            "free",
            &platform_store::BotCredential {
                client_id: "111".to_string(),
                bot_name: "x".to_string(),
                token_sealed: sealed,
            },
        )
    });
    assert!(matches!(saved, Ok(Ok(()))));
    let r = app
        .post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=free&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(r.body.contains("Save the token again"));
    assert!(deployed(&app).is_empty());
}

#[tokio::test]
async fn a_docker_failure_is_reported_without_details() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-discord-secret", "111");
    if let Ok(mut fail) = app.containers.fail_deploy.lock() {
        *fail = true;
    }
    let r = app
        .post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=free&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.body.contains("Docker did not start the container"));
    assert!(deployed(&app).is_empty());
}

#[tokio::test]
async fn nobody_else_can_deploy_or_stop_my_bot() {
    let app = App::new(true, false);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, b_csrf) = app.dev_login("bob").await;
    create(&app, &alice, &a_csrf, "Alice Bot").await;
    let deploy = app
        .post(
            "/products/1/deploy",
            Some(&bob),
            &format!("tier=free&csrf={b_csrf}"),
        )
        .await;
    assert_eq!(deploy.status, StatusCode::NOT_FOUND);
    let stop = app
        .post(
            "/products/1/stop",
            Some(&bob),
            &format!("tier=free&csrf={b_csrf}"),
        )
        .await;
    assert_eq!(stop.status, StatusCode::NOT_FOUND);
    assert!(deployed(&app).is_empty());
    assert!(app
        .containers
        .stopped
        .lock()
        .map(|s| s.is_empty())
        .unwrap_or(false));
}

#[tokio::test]
async fn deploy_and_stop_check_the_token_and_the_tier() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    for action in ["deploy", "stop"] {
        let path = format!("/products/1/{action}");
        assert_eq!(
            app.post(&path, Some(&alice), "tier=free&csrf=wrong")
                .await
                .status,
            StatusCode::FORBIDDEN,
            "{action}"
        );
        assert_eq!(
            app.post(&path, Some(&alice), &format!("tier=gold&csrf={csrf}"))
                .await
                .status,
            StatusCode::BAD_REQUEST,
            "{action}"
        );
    }
    assert!(deployed(&app).is_empty());
}

#[tokio::test]
async fn stopping_removes_the_right_container() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    let r = app
        .post(
            "/products/1/stop",
            Some(&alice),
            &format!("tier=premium&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1"));
    let stopped = app
        .containers
        .stopped
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    assert_eq!(stopped, ["bot-1-premium"]);
}

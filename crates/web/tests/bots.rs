mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use support::*;

const TOKEN: &str = "MTIzNDU2.GhIJkl.abc-_DEF";

async fn scene(cooldown: u32) -> (App, String, String) {
    let app = App::with_limits(true, false, 10, cooldown);
    let (alice, csrf) = app.dev_login("alice").await;
    let r = app
        .post(
            "/products",
            Some(&alice),
            &format!("name=Alice+Bot&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1"));
    (app, alice, csrf)
}

async fn save(app: &App, cookie: &str, csrf: &str, tier: &str, token: &str) -> Resp {
    app.post(
        "/products/1/bot",
        Some(cookie),
        &format!("tier={tier}&token={}&csrf={csrf}", form_encode(token)),
    )
    .await
}

#[tokio::test]
async fn saving_a_token_reads_the_application_from_discord_and_stores_it_encrypted() {
    let (app, alice, csrf) = scene(0).await;
    app.bots.know(TOKEN, "1234567895647001626", "Alice Bot App");
    let r = save(&app, &alice, &csrf, "free", &format!("  {TOKEN}  ")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Bot connected"));
    assert!(!r.body.contains(TOKEN));
    assert_eq!(app.bots.asked(), [TOKEN]);

    let Some(stored) = app.stored_credential(1, "free") else {
        panic!("nothing stored");
    };
    assert_eq!(stored.client_id, "1234567895647001626");
    assert_eq!(stored.bot_name, "Alice Bot App");
    assert!(!stored
        .token_sealed
        .windows(TOKEN.len())
        .any(|w| w == TOKEN.as_bytes()));
    let opened = app
        .vault
        .open(&stored.token_sealed, "bot-credential:1:free")
        .ok();
    assert_eq!(opened.as_deref(), Some(TOKEN));
    assert!(app.stored_credential(1, "premium").is_none());

    let page = app.get("/products/1/bot", Some(&alice)).await.body;
    assert!(page.contains("Alice Bot App") && page.contains("1234567895647001626"));
    assert!(page.contains("client_id=1234567895647001626"));
    assert!(page.contains("Replace the bot token") && page.contains(">Bot token</label>"));
    assert!(!page.contains(TOKEN));
}

#[tokio::test]
async fn the_token_field_is_a_password_input_posted_never_in_the_address() {
    let (app, alice, _) = scene(0).await;
    let page = app.get("/products/1/bot", Some(&alice)).await.body;
    assert!(page.contains(r#"type="password" name="token""#), "{page}");
    assert!(page.contains(r#"autocomplete="off""#));
    assert!(page.contains(r#"method="post" action="/products/1/bot""#));
}

#[tokio::test]
async fn a_saved_token_is_what_the_container_gets_when_deployed() {
    let (app, alice, csrf) = scene(0).await;
    app.bots.know(TOKEN, "111", "A");
    save(&app, &alice, &csrf, "premium", TOKEN).await;
    let r = app
        .post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=premium&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let specs = app
        .containers
        .deployed
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    let token = specs[0]
        .env
        .iter()
        .find(|(n, _)| n == "DISCORD_TOKEN")
        .map(|(_, v)| v.as_str());
    assert_eq!(token, Some(TOKEN));
    assert!(!r.body.contains(TOKEN));
}

#[tokio::test]
async fn only_the_owner_can_connect_a_bot_and_discord_is_not_asked_otherwise() {
    let (app, alice, csrf) = scene(0).await;
    let (bob, bob_csrf) = app.dev_login("bob").await;
    app.bots.know(TOKEN, "111", "A");
    let r = save(&app, &bob, &bob_csrf, "free", TOKEN).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let wrong_csrf = save(&app, &alice, "wrong", "free", TOKEN).await;
    assert_eq!(wrong_csrf.status, StatusCode::FORBIDDEN);
    let anonymous = app
        .post(
            "/products/1/bot",
            None,
            &format!("tier=free&token={TOKEN}&csrf={csrf}"),
        )
        .await;
    assert_ne!(anonymous.status, StatusCode::OK);
    let tier = save(&app, &alice, &csrf, "gold", TOKEN).await;
    assert_eq!(tier.status, StatusCode::BAD_REQUEST);
    assert!(app.bots.asked().is_empty());
    assert!(app.stored_credential(1, "free").is_none());
}

#[tokio::test]
async fn malformed_tokens_are_refused_before_asking_discord() {
    let (app, alice, csrf) = scene(0).await;
    for bad in ["", "   ", "two words", "tok\u{e9}n", &"a".repeat(201)] {
        let r = save(&app, &alice, &csrf, "free", bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}");
        assert!(r.body.contains("Invalid token"), "{bad:?}");
    }
    assert!(app.bots.asked().is_empty());
}

#[tokio::test]
async fn a_token_discord_refuses_is_not_stored_and_not_echoed() {
    let (app, alice, csrf) = scene(0).await;
    let r = save(&app, &alice, &csrf, "free", "not.a.real.token").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("Discord refused this token"));
    assert!(!r.body.contains("not.a.real.token"));
    assert!(app.stored_credential(1, "free").is_none());
}

#[tokio::test]
async fn when_discord_is_down_nothing_is_stored_and_the_message_says_to_retry() {
    let (app, alice, csrf) = scene(0).await;
    app.bots.know(TOKEN, "111", "A");
    if let Ok(mut down) = app.bots.down.lock() {
        *down = true;
    }
    let r = save(&app, &alice, &csrf, "free", TOKEN).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.body.contains("Discord did not answer"));
    assert!(!r.body.contains(TOKEN));
    assert!(app.stored_credential(1, "free").is_none());
}

#[tokio::test]
async fn replacing_a_token_updates_the_application_and_the_name() {
    let (app, alice, csrf) = scene(0).await;
    app.bots.know("first.tok.en", "111", "First");
    app.bots.know("second.tok.en", "222", "Second");
    save(&app, &alice, &csrf, "free", "first.tok.en").await;
    let r = save(&app, &alice, &csrf, "free", "second.tok.en").await;
    assert_eq!(r.status, StatusCode::OK);
    let Some(stored) = app.stored_credential(1, "free") else {
        panic!("nothing stored");
    };
    assert_eq!(
        (stored.client_id.as_str(), stored.bot_name.as_str()),
        ("222", "Second")
    );
    let opened = app
        .vault
        .open(&stored.token_sealed, "bot-credential:1:free")
        .ok();
    assert_eq!(opened.as_deref(), Some("second.tok.en"));
}

#[tokio::test]
async fn one_discord_application_cannot_be_used_twice() {
    let (app, alice, csrf) = scene(0).await;
    app.bots.know("a.tok.en", "111", "App");
    app.bots.know("a2.tok.en", "111", "App again");
    assert_eq!(
        save(&app, &alice, &csrf, "free", "a.tok.en").await.status,
        StatusCode::OK
    );
    let same_product = save(&app, &alice, &csrf, "premium", "a2.tok.en").await;
    assert_eq!(same_product.status, StatusCode::CONFLICT);
    assert!(same_product.body.contains("Application already used"));
    assert!(app.stored_credential(1, "premium").is_none());

    let (bob, bob_csrf) = app.dev_login("bob").await;
    let made = app
        .post(
            "/products",
            Some(&bob),
            &format!("name=Bob+Bot&csrf={bob_csrf}"),
        )
        .await;
    assert_eq!(made.location(), Some("/products/2"));
    let stolen = app
        .post(
            "/products/2/bot",
            Some(&bob),
            &format!("tier=free&token=a2.tok.en&csrf={bob_csrf}"),
        )
        .await;
    assert_eq!(stolen.status, StatusCode::CONFLICT);
    assert!(app.stored_credential(2, "free").is_none());
    assert_eq!(
        app.stored_credential(1, "free").map(|c| c.bot_name),
        Some("App".to_string())
    );
}

#[tokio::test]
async fn checks_are_rate_limited_per_version() {
    let (app, alice, csrf) = scene(30).await;
    app.bots.know("a.tok.en", "111", "A");
    app.bots.know("b.tok.en", "222", "B");
    assert_eq!(
        save(&app, &alice, &csrf, "free", "a.tok.en").await.status,
        StatusCode::OK
    );
    let again = save(&app, &alice, &csrf, "free", "a.tok.en").await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(again.body.contains("Wait 30 more second(s)"));
    assert_eq!(app.bots.asked().len(), 1);
    assert_eq!(
        save(&app, &alice, &csrf, "premium", "b.tok.en")
            .await
            .status,
        StatusCode::OK
    );
    app.advance(30);
    assert_eq!(
        save(&app, &alice, &csrf, "free", "a.tok.en").await.status,
        StatusCode::OK
    );
    assert_eq!(app.bots.asked().len(), 3);
}

#[tokio::test]
async fn a_sealed_token_copied_to_another_version_cannot_be_used() {
    let (app, alice, csrf) = scene(0).await;
    app.seed_bot(1, "free", "free-tok", "111");
    let Some(free) = app.stored_credential(1, "free") else {
        panic!("nothing stored");
    };
    let copy = platform_store::BotCredential {
        client_id: "222".to_string(),
        ..free
    };
    let saved = app
        .store
        .lock()
        .map(|s| s.save_credential(ProductId(1), "premium", &copy));
    assert!(matches!(saved, Ok(Ok(()))));
    let r = app
        .post(
            "/products/1/deploy",
            Some(&alice),
            &format!("tier=premium&csrf={csrf}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(r.body.contains("Save the token again"));
}

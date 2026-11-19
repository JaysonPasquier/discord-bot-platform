mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::Plan;
use platform_web::{ProfileError, ProfilePatch};
use support::*;

const BOB_GUILD: &str = "2000000000000000002";
const BOB: u64 = 1_000_000_000_000_000_002;
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
const PATH: &str = "/subscriptions/1/2000000000000000002";
const APPLY: &str = "/subscriptions/1/2000000000000000002/profile";
const PREMIUM_TOKEN: &str = "premium.bot.token";

type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

struct Scene {
    app: App,
    bob: (String, String),
}

// Alice crée le bot 1 avec une version premium ; Bob souscrit pour son serveur.
async fn scene(cooldown: u32) -> Scene {
    let app = App::with_limits(true, false, 10, cooldown);
    let alice = app.dev_login("alice").await;
    let bob = app.dev_login("bob").await;
    app.post(
        "/products",
        Some(&alice.0),
        &format!("name=Alice+Bot&csrf={}", alice.1),
    )
    .await;
    app.seed_bot(1, "free", "free.bot.token", "111");
    app.seed_bot(1, "premium", PREMIUM_TOKEN, "222");
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
    let r = app
        .post(
            "/subscribe",
            Some(&bob.0),
            &format!("product=1&guild={BOB_GUILD}&csrf={}", bob.1),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(r.location().unwrap_or_default().starts_with("https://"));
    let completed = stripe_checkout_completed(
        BOB_GUILD,
        1,
        BOB,
        30,
        "Pro",
        500,
        app.now.load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(app.stripe_webhook(&completed).await, StatusCode::OK);
    Scene { app, bob }
}

fn text<'a>(name: &'a str, value: &'a str) -> Part<'a> {
    (name, None, value.as_bytes())
}

async fn apply(s: &Scene, parts: &[Part<'_>]) -> Resp {
    s.app.post_multipart(APPLY, Some(&s.bob.0), parts).await
}

#[tokio::test]
async fn the_dashboard_offers_to_customize_an_active_subscription() {
    let s = scene(0).await;
    let page = s.app.get("/servers", Some(&s.bob.0)).await.body;
    assert!(
        page.contains(&format!(r#"href="{PATH}">Customize</a>"#)),
        "{page}"
    );
}

#[tokio::test]
async fn the_page_has_the_fields_and_names_the_server_and_the_bot() {
    let s = scene(0).await;
    let r = s.app.get(PATH, Some(&s.bob.0)).await;
    assert_eq!(r.status, StatusCode::OK);
    for needle in [
        "Customize Alice Bot",
        "Bob Dev Server A",
        "Bot 222",
        r#"enctype="multipart/form-data""#,
        r#"name="nick""#,
        r#"name="bio""#,
        r#"type="file" name="avatar""#,
        r#"type="file" name="banner""#,
        r#"name="remove_avatar""#,
        "client_id=222",
    ] {
        assert!(r.body.contains(needle), "{needle}");
    }
    assert!(!r.body.contains(PREMIUM_TOKEN));
}

#[tokio::test]
async fn applying_calls_discord_with_the_creators_premium_token_for_that_server() {
    let s = scene(0).await;
    let csrf = &s.bob.1;
    let r = apply(
        &s,
        &[
            text("csrf", csrf),
            text("nick", "  Rex  "),
            text("bio", "Guard dog"),
            ("avatar", Some("a.png"), PNG),
            ("banner", Some(""), b""),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), Some(PATH));
    let calls = s.app.profiles.calls();
    assert_eq!(calls.len(), 1);
    let (token, guild, patch) = &calls[0];
    assert_eq!(token, PREMIUM_TOKEN);
    assert_eq!(*guild, 2_000_000_000_000_000_002);
    assert_eq!(patch.nick, Some(Some("Rex".to_string())));
    assert_eq!(patch.bio, Some(Some("Guard dog".to_string())));
    assert!(patch
        .avatar
        .as_ref()
        .and_then(|a| a.as_deref())
        .is_some_and(|a| a.starts_with("data:image/png;base64,iVBORw0KGgo")));
    assert_eq!(patch.banner, None);
}

#[tokio::test]
async fn the_page_then_shows_what_was_applied() {
    let s = scene(0).await;
    apply(
        &s,
        &[
            text("csrf", &s.bob.1),
            text("nick", "Rex"),
            text("bio", "Guard <dog>"),
            ("banner", Some("b.png"), PNG),
        ],
    )
    .await;
    let body = s.app.get(PATH, Some(&s.bob.0)).await.body;
    assert!(body.contains(r#"value="Rex""#));
    assert!(body.contains("Guard &lt;dog&gt;") && !body.contains("Guard <dog>"));
    assert!(
        body.contains("Banner is currently custom")
            && body.contains("Picture is currently default")
    );
}

#[tokio::test]
async fn empty_fields_reset_to_default_and_the_remove_box_clears_an_image() {
    let s = scene(0).await;
    apply(
        &s,
        &[
            text("csrf", &s.bob.1),
            text("nick", "Rex"),
            ("avatar", Some("a.png"), PNG),
        ],
    )
    .await;
    let r = apply(
        &s,
        &[
            text("csrf", &s.bob.1),
            text("nick", ""),
            text("bio", "   "),
            text("remove_avatar", "1"),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let calls = s.app.profiles.calls();
    let patch = &calls[1].2;
    assert_eq!(
        *patch,
        ProfilePatch {
            nick: Some(None),
            bio: Some(None),
            avatar: Some(None),
            banner: None,
        }
    );
    let body = s.app.get(PATH, Some(&s.bob.0)).await.body;
    assert!(body.contains("Picture is currently default"));
}

#[tokio::test]
async fn an_image_left_alone_stays_marked_as_custom() {
    let s = scene(0).await;
    apply(
        &s,
        &[text("csrf", &s.bob.1), ("avatar", Some("a.png"), PNG)],
    )
    .await;
    apply(&s, &[text("csrf", &s.bob.1), text("nick", "Rex")]).await;
    let body = s.app.get(PATH, Some(&s.bob.0)).await.body;
    assert!(body.contains("Picture is currently custom"));
    assert_eq!(s.app.profiles.calls()[1].2.avatar, None);
}

#[tokio::test]
async fn only_the_buyer_of_that_server_can_see_or_change_it() {
    let s = scene(0).await;
    let (alice, a_csrf) = s.app.dev_login("alice").await;
    assert_eq!(
        s.app.get(PATH, Some(&alice)).await.status,
        StatusCode::NOT_FOUND
    );
    let r = s
        .app
        .post_multipart(
            APPLY,
            Some(&alice),
            &[text("csrf", &a_csrf), text("nick", "Hacked")],
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let other_guild = s
        .app
        .post_multipart(
            "/subscriptions/1/2000000000000000003/profile",
            Some(&s.bob.0),
            &[text("csrf", &s.bob.1), text("nick", "x")],
        )
        .await;
    assert_eq!(other_guild.status, StatusCode::NOT_FOUND);
    let other_product = s
        .app
        .get("/subscriptions/2/2000000000000000002", Some(&s.bob.0))
        .await;
    assert_eq!(other_product.status, StatusCode::NOT_FOUND);
    assert_ne!(s.app.get(PATH, None).await.status, StatusCode::OK);
    let anonymous = s
        .app
        .post_multipart(APPLY, None, &[text("nick", "x")])
        .await;
    assert_ne!(anonymous.status, StatusCode::OK);
    assert!(s.app.profiles.calls().is_empty());
}

#[tokio::test]
async fn the_form_token_is_required() {
    let s = scene(0).await;
    for parts in [
        vec![text("csrf", "wrong"), text("nick", "x")],
        vec![text("nick", "x")],
    ] {
        let r = apply(&s, &parts).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    assert!(s.app.profiles.calls().is_empty());
}

#[tokio::test]
async fn an_expired_or_canceled_subscription_cannot_customize() {
    let s = scene(0).await;
    s.app
        .post(
            "/subscriptions/cancel",
            Some(&s.bob.0),
            &format!("product=1&guild={BOB_GUILD}&csrf={}", s.bob.1),
        )
        .await;
    let page = s.app.get(PATH, Some(&s.bob.0)).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    assert!(page.body.contains("Subscription not active"));
    let r = apply(&s, &[text("csrf", &s.bob.1), text("nick", "x")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(s.app.profiles.calls().is_empty());
    let dashboard = s.app.get("/servers", Some(&s.bob.0)).await.body;
    assert!(!dashboard.contains(">Customize</a>"));
}

#[tokio::test]
async fn bad_texts_and_images_are_refused_before_discord_is_called() {
    let s = scene(0).await;
    let too_long = "n".repeat(33);
    let bio_long = "b".repeat(191);
    let big = {
        let mut v = PNG.to_vec();
        v.resize(2 * 1024 * 1024 + 1, 0);
        v
    };
    let cases: Vec<(Vec<Part<'_>>, &str)> = vec![
        (vec![text("nick", &too_long)], "Invalid text"),
        (vec![text("bio", &bio_long)], "Invalid text"),
        (vec![text("nick", "a\u{7}b")], "Invalid text"),
        (
            vec![("avatar", Some("a.png"), b"<svg onload=alert(1)>")],
            "Only PNG, JPEG and GIF",
        ),
        (
            vec![("banner", Some("b.png"), big.as_slice())],
            "larger than 2 MB",
        ),
    ];
    for (mut parts, expected) in cases {
        parts.insert(0, text("csrf", &s.bob.1));
        let r = apply(&s, &parts).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{expected}");
        assert!(r.body.contains(expected), "{expected}: {}", r.body);
    }
    assert!(s.app.profiles.calls().is_empty());
}

#[tokio::test]
async fn each_discord_failure_gets_a_clear_message_and_nothing_is_saved() {
    let s = scene(0).await;
    for (error, status, title) in [
        (
            ProfileError::NotInServer,
            StatusCode::CONFLICT,
            "The bot is not in this server",
        ),
        (
            ProfileError::Forbidden,
            StatusCode::FORBIDDEN,
            "Discord refused the change",
        ),
        (
            ProfileError::Invalid,
            StatusCode::BAD_REQUEST,
            "Discord refused these values",
        ),
        (
            ProfileError::RateLimited,
            StatusCode::TOO_MANY_REQUESTS,
            "Too many changes",
        ),
        (
            ProfileError::BadToken,
            StatusCode::BAD_GATEWAY,
            "The bot cannot be reached",
        ),
        (
            ProfileError::Unavailable,
            StatusCode::BAD_GATEWAY,
            "Discord did not answer",
        ),
    ] {
        s.app.profiles.fail_with(error);
        let r = apply(&s, &[text("csrf", &s.bob.1), text("nick", "Rex")]).await;
        assert_eq!(r.status, status, "{error:?}");
        assert!(r.body.contains(title), "{error:?}");
        assert!(!r.body.contains(PREMIUM_TOKEN));
    }
    let body = s.app.get(PATH, Some(&s.bob.0)).await.body;
    assert!(!body.contains(r#"value="Rex""#));
}

#[tokio::test]
async fn changes_are_rate_limited_per_server() {
    let s = scene(30).await;
    let go = || async { apply(&s, &[text("csrf", &s.bob.1), text("nick", "Rex")]).await };
    assert_eq!(go().await.status, StatusCode::SEE_OTHER);
    let again = go().await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(again.body.contains("Wait 30 more second(s)"));
    assert_eq!(s.app.profiles.calls().len(), 1);
    s.app.advance(30);
    assert_eq!(go().await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn without_a_premium_bot_there_is_nothing_to_customize() {
    let app = App::new(true, false);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, b_csrf) = app.dev_login("bob").await;
    app.post(
        "/products",
        Some(&alice),
        &format!("name=Alice+Bot&csrf={a_csrf}"),
    )
    .await;
    app.seed_bot(1, "premium", "tok", "222");
    app.post(
        "/subscribe",
        Some(&bob),
        &format!("product=1&guild={BOB_GUILD}&csrf={b_csrf}"),
    )
    .await;
    let other = App::new(true, false);
    let (o_alice, o_csrf) = other.dev_login("alice").await;
    other
        .post(
            "/products",
            Some(&o_alice),
            &format!("name=Free+Only&csrf={o_csrf}"),
        )
        .await;
    other.seed_bot(1, "free", "tok", "111");
    assert!(other.store.lock().is_ok_and(|store| store
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
    let (o_bob, o_bob_csrf) = other.dev_login("bob").await;
    let subscribed = other
        .post(
            "/subscribe",
            Some(&o_bob),
            &format!("product=1&guild={BOB_GUILD}&csrf={o_bob_csrf}"),
        )
        .await;
    assert_eq!(subscribed.status, StatusCode::SEE_OTHER);
    let completed = stripe_checkout_completed(
        BOB_GUILD,
        1,
        BOB,
        30,
        "Pro",
        500,
        other.now.load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(other.stripe_webhook(&completed).await, StatusCode::OK);
    let page = other.get(PATH, Some(&o_bob)).await;
    assert_eq!(page.status, StatusCode::CONFLICT);
    assert!(page.body.contains("Premium bot not available"));
}

#[tokio::test]
async fn a_stored_token_that_cannot_be_read_asks_the_creator_to_paste_it_again() {
    let s = scene(0).await;
    let other = platform_store::Vault::new(&[1; 32]);
    let Ok(sealed) = other.seal(PREMIUM_TOKEN, "bot-credential:1:premium") else {
        panic!("cannot seal");
    };
    let saved = s.app.store.lock().map(|st| {
        st.save_credential(
            platform_core::ProductId(1),
            "premium",
            &platform_store::BotCredential {
                client_id: "222".to_string(),
                bot_name: "x".to_string(),
                token_sealed: sealed,
            },
        )
    });
    assert!(matches!(saved, Ok(Ok(()))));
    let r = apply(&s, &[text("csrf", &s.bob.1), text("nick", "x")]).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(r.body.contains("Stored token unreadable"));
    assert!(s.app.profiles.calls().is_empty());
}

#[tokio::test]
async fn an_oversized_upload_is_cut_off_by_the_body_limit() {
    let s = scene(0).await;
    let huge = vec![0u8; 6 * 1024 * 1024];
    let r = apply(
        &s,
        &[
            text("csrf", &s.bob.1),
            ("avatar", Some("a.png"), huge.as_slice()),
        ],
    )
    .await;
    assert!(
        r.status == StatusCode::BAD_REQUEST || r.status == StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        r.status
    );
    assert!(s.app.profiles.calls().is_empty());
}

#[tokio::test]
async fn the_banner_can_be_removed_without_touching_the_avatar() {
    let s = scene(0).await;
    apply(
        &s,
        &[
            text("csrf", &s.bob.1),
            ("avatar", Some("a.png"), PNG),
            ("banner", Some("b.png"), PNG),
        ],
    )
    .await;
    let r = apply(&s, &[text("csrf", &s.bob.1), text("remove_banner", "1")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let patch = &s.app.profiles.calls()[1].2;
    assert_eq!(
        (patch.avatar.clone(), patch.banner.clone()),
        (None, Some(None))
    );
    let body = s.app.get(PATH, Some(&s.bob.0)).await.body;
    assert!(
        body.contains("Picture is currently custom")
            && body.contains("Banner is currently default")
    );
}

mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::Plan;
use platform_web::ProfileError;
use support::*;

// dev_user("bob") retourne guild("2000000000000000002", "Bob Dev Server A").
const BOB_GUILD: &str = "2000000000000000002";
const BOB: u64 = 1_000_000_000_000_000_002;
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
const API_GET: &str = "/api/subscriptions/1/2000000000000000002";
const API_APPLY: &str = "/api/subscriptions/1/2000000000000000002/profile";
const PREMIUM_TOKEN: &str = "premium.bot.token";

type Part<'a> = (&'a str, Option<&'a str>, &'a [u8]);

struct Scene {
    app: App,
    bob: (String, String),
}

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
    assert!(app.store.lock().is_ok_and(|s| s
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
        .api(
            "POST",
            "/api/subscribe",
            Some(&bob.0),
            Some(&bob.1),
            Some(&format!(r#"{{"product":1,"guild":"{BOB_GUILD}"}}"#)),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "subscribe: {}", r.body);
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

async fn api_apply(s: &Scene, parts: &[Part<'_>]) -> Resp {
    s.app
        .api_multipart(API_APPLY, &s.bob.0, &s.bob.1, parts)
        .await
}

#[tokio::test]
async fn get_returns_the_bot_name_and_no_profile_yet() {
    let s = scene(0).await;
    let r = s.app.api("GET", API_GET, Some(&s.bob.0), None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(body["bot_name"], "Bot 222");
    assert_eq!(body["avatar_set"], false);
    assert_eq!(body["nick"], serde_json::Value::Null);
}

#[tokio::test]
async fn get_requires_a_session() {
    let s = scene(0).await;
    let r = s.app.api("GET", API_GET, None, None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_404_when_not_subscribed() {
    let s = scene(0).await;
    let other = s.app.dev_login("alice").await;
    let r = s.app.api("GET", API_GET, Some(&other.0), None, None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn apply_saves_the_profile_and_calls_discord() {
    let s = scene(0).await;
    let r = api_apply(&s, &[text("nick", "Rex"), text("bio", "A good bot.")]).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);

    let calls = s.app.profiles.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, PREMIUM_TOKEN);
    assert_eq!(calls[0].1, BOB_GUILD.parse::<u64>().unwrap());
    assert_eq!(calls[0].2.nick, Some(Some("Rex".to_string())));
    assert_eq!(calls[0].2.bio, Some(Some("A good bot.".to_string())));
}

#[tokio::test]
async fn apply_requires_csrf_token() {
    let s = scene(0).await;
    let r = s
        .app
        .api_multipart(API_APPLY, &s.bob.0, "wrong-token", &[text("nick", "Rex")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn apply_saves_the_avatar_when_a_png_is_uploaded() {
    let s = scene(0).await;
    let r = api_apply(
        &s,
        &[
            text("nick", "Rex"),
            text("bio", ""),
            ("avatar", Some("avatar.png"), PNG),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let calls = s.app.profiles.calls();
    assert!(calls[0].2.avatar.as_ref().is_some_and(|a| a.is_some()));

    // Vérifie que le profil enregistré reflète avatar_set = true.
    let r2 = s.app.api("GET", API_GET, Some(&s.bob.0), None, None).await;
    let body: serde_json::Value = serde_json::from_str(&r2.body).unwrap();
    assert_eq!(body["avatar_set"], true);
}

#[tokio::test]
async fn apply_removes_the_avatar_when_the_remove_field_is_present() {
    let s = scene(0).await;
    // D'abord on pose l'avatar.
    api_apply(
        &s,
        &[
            text("nick", "Rex"),
            text("bio", ""),
            ("avatar", Some("a.png"), PNG),
        ],
    )
    .await;

    // Ensuite on l'enlève.
    let r = api_apply(
        &s,
        &[
            text("nick", "Rex"),
            text("bio", ""),
            text("remove_avatar", "1"),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let calls = s.app.profiles.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].2.avatar, Some(None));
    let r2 = s.app.api("GET", API_GET, Some(&s.bob.0), None, None).await;
    let body: serde_json::Value = serde_json::from_str(&r2.body).unwrap();
    assert_eq!(body["avatar_set"], false);
}

#[tokio::test]
async fn apply_rejects_names_longer_than_32_chars() {
    let s = scene(0).await;
    let r = api_apply(&s, &[text("nick", &"a".repeat(33)), text("bio", "")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(body["error"]["code"], "invalid_text");
}

#[tokio::test]
async fn apply_forwards_discord_errors() {
    let s = scene(0).await;
    s.app.profiles.fail_with(ProfileError::NotInServer);
    let r = api_apply(&s, &[text("nick", "Rex"), text("bio", "")]).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(body["error"]["code"], "bot_not_in_server");
}

#[tokio::test]
async fn apply_respects_the_rate_limit() {
    let s = scene(60).await;
    let r1 = api_apply(&s, &[text("nick", "Rex"), text("bio", "")]).await;
    assert_eq!(r1.status, StatusCode::NO_CONTENT);
    let r2 = api_apply(&s, &[text("nick", "Rex"), text("bio", "")]).await;
    assert_eq!(r2.status, StatusCode::TOO_MANY_REQUESTS);
    let body: serde_json::Value = serde_json::from_str(&r2.body).unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");
}

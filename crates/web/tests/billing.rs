mod support;

use axum::http::StatusCode;
use platform_core::{PremiumStatus, ProductId};
use platform_store::Plan;
use support::*;

const DAY: i64 = 86_400;
const BOB_GUILD: &str = "2000000000000000002";
const BOB_GUILD_ID: u64 = 2_000_000_000_000_000_002;
const BOB: u64 = 1_000_000_000_000_000_002;
const ALICE_GUILD: &str = "2000000000000000001";
const PLAN_DAYS: u32 = 30;
const PLAN_CENTS: u32 = 500;

struct Scene {
    app: App,
    alice: (String, String),
    bob: (String, String),
}

// Alice crée le bot 1 ; Bob est l'acheteur. Un plan tarifé est posé : sans lui,
// s'abonner est refusé.
async fn scene() -> Scene {
    let app = App::new(true, false);
    let alice = app.dev_login("alice").await;
    let bob = app.dev_login("bob").await;
    let r = app
        .post(
            "/products",
            Some(&alice.0),
            &format!("name=Alice+Bot&csrf={}", alice.1),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1"));
    app.seed_bot(1, "free", "free-secret", "111");
    app.seed_bot(1, "premium", "premium-secret", "222");
    assert!(app.store.lock().is_ok_and(|store| store
        .replace_plans(
            ProductId(1),
            &[Plan {
                id: 0,
                name: "Pro".to_string(),
                price_cents: PLAN_CENTS,
                days: PLAN_DAYS
            }]
        )
        .is_ok()));
    Scene { app, alice, bob }
}

// Poste le formulaire (ouvre une session Stripe Checkout) puis simule le paiement confirmé par
// le webhook : comme un vrai abonnement de bout en bout.
async fn subscribe(s: &Scene, guild: &str, csrf: &str) -> Resp {
    let r = s
        .app
        .post(
            "/subscribe",
            Some(&s.bob.0),
            &format!("product=1&guild={guild}&csrf={csrf}"),
        )
        .await;
    if r.status == StatusCode::SEE_OTHER {
        let at = s.app.now.load(std::sync::atomic::Ordering::SeqCst);
        let completed = stripe_checkout_completed(guild, 1, BOB, PLAN_DAYS, "Pro", PLAN_CENTS, at);
        assert_eq!(s.app.stripe_webhook(&completed).await, StatusCode::OK);
    }
    r
}

fn status(s: &Scene) -> Option<PremiumStatus> {
    s.app
        .entitlement(BOB_GUILD_ID, 1)
        .map(|e| e.status(s.app.now.load(std::sync::atomic::Ordering::SeqCst), 259_200))
}

#[tokio::test]
async fn the_market_lists_bots_with_a_subscribe_form_for_my_servers() {
    let s = scene().await;
    let market = s.app.get("/market", Some(&s.bob.0)).await;
    assert_eq!(market.status, StatusCode::OK);
    assert!(market.body.contains("Alice Bot") && market.body.contains(r#"href="/bots/1""#));
    let page = s.app.get("/bots/1", Some(&s.bob.0)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Alice Bot"));
    assert!(page.body.contains("Bob Dev Server A") && page.body.contains("Bob Dev Server B"));
    assert!(!page.body.contains("Alice Dev Server"));
    assert!(page.body.contains("Subscribe 30 days"));
}

#[tokio::test]
async fn the_market_links_each_version_to_the_creators_own_application() {
    let s = scene().await;
    let body = s.app.get("/bots/1", Some(&s.bob.0)).await.body;
    assert!(body.contains("Add the free bot") && body.contains("client_id=111"));
    assert!(body.contains("Add the premium bot") && body.contains("client_id=222"));
    assert!(!body.contains("free-secret") && !body.contains("premium-secret"));
}

#[tokio::test]
async fn a_bot_without_any_connected_version_is_not_on_the_market() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    let (bob, _) = app.dev_login("bob").await;
    app.post(
        "/products",
        Some(&alice),
        &format!("name=Hidden+Bot&csrf={csrf}"),
    )
    .await;
    assert!(!app
        .get("/market", Some(&bob))
        .await
        .body
        .contains("Hidden Bot"));
    app.seed_bot(1, "free", "t", "111");
    assert!(app
        .get("/market", Some(&bob))
        .await
        .body
        .contains("Hidden Bot"));
    let page = app.get("/bots/1", Some(&bob)).await.body;
    assert!(page.contains("Hidden Bot") && page.contains("Add the free bot"));
    assert!(!page.contains("Add the premium bot"));
    assert!(page.contains("No premium version yet") && !page.contains("Subscribe 30 days"));
}

#[tokio::test]
async fn subscribing_activates_premium_for_thirty_days() {
    let s = scene().await;
    let r = subscribe(&s, BOB_GUILD, &s.bob.1).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(r.location().unwrap_or_default().starts_with("https://"));
    let e = s.app.entitlement(BOB_GUILD_ID, 1);
    assert_eq!(e.map(|e| e.paid_until), Some(START + 30 * DAY));
    assert_eq!(status(&s), Some(PremiumStatus::Active));
    let dashboard = s.app.get("/servers", Some(&s.bob.0)).await;
    assert!(dashboard.body.contains("Alice Bot") && dashboard.body.contains("Bob Dev Server A"));
    assert!(dashboard
        .body
        .contains(r#"<span class="chip chip-ok">Active</span>"#));
    assert!(dashboard.body.contains("2023-12-14"));
}

#[tokio::test]
async fn subscribing_again_extends_from_the_end_of_the_period() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    s.app.advance(5 * DAY);
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    assert_eq!(
        s.app.entitlement(BOB_GUILD_ID, 1).map(|e| e.paid_until),
        Some(START + 60 * DAY)
    );
}

#[tokio::test]
async fn a_server_i_do_not_manage_is_refused() {
    let s = scene().await;
    for guild in [ALICE_GUILD, "999"] {
        let r = subscribe(&s, guild, &s.bob.1).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{guild}");
    }
    assert!(s.app.entitlement(1_000, 1).is_none());
    assert!(s.app.entitlement(2_000_000_000_000_000_001, 1).is_none());
    assert!(s.app.entitlement(999, 1).is_none());
}

#[tokio::test]
async fn malformed_subscriptions_are_rejected() {
    let s = scene().await;
    assert_eq!(
        subscribe(&s, "abc", &s.bob.1).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        subscribe(&s, "%2B2000000000000000002", &s.bob.1)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        subscribe(&s, BOB_GUILD, "wrong").await.status,
        StatusCode::FORBIDDEN
    );
    let unknown = s
        .app
        .post(
            "/subscribe",
            Some(&s.bob.0),
            &format!("product=42&guild={BOB_GUILD}&csrf={}", s.bob.1),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert!(s.app.entitlement(BOB_GUILD_ID, 1).is_none());
    assert!(s.app.entitlement(BOB_GUILD_ID, 42).is_none());
}

#[tokio::test]
async fn the_status_follows_the_period_then_the_grace_then_expires() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    s.app.advance(30 * DAY);
    let (cookie, _) = s.app.dev_login("bob").await;
    let page = s.app.get("/servers", Some(&cookie)).await;
    assert!(page
        .body
        .contains(r#"<span class="chip chip-warn">Ending soon</span>"#));
    s.app.advance(3 * DAY);
    let (cookie, _) = s.app.dev_login("bob").await;
    let page = s.app.get("/servers", Some(&cookie)).await;
    assert!(page.body.contains(r#"<span class="chip">Expired</span>"#));
    assert!(!page.body.contains("Cancel"));
}

#[tokio::test]
async fn the_subscriber_can_cancel_and_access_ends_at_once() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    let before = s.app.get("/servers", Some(&s.bob.0)).await;
    assert!(before.body.contains("Cancel"));
    let body = format!("product=1&guild={BOB_GUILD}&csrf={}", s.bob.1);
    let r = s
        .app
        .post("/subscriptions/cancel", Some(&s.bob.0), &body)
        .await;
    assert_eq!(r.location(), Some("/dashboard"));
    assert_eq!(
        s.app.entitlement(BOB_GUILD_ID, 1).map(|e| e.canceled),
        Some(true)
    );
    assert_eq!(status(&s), Some(PremiumStatus::Expired));
    let after = s.app.get("/servers", Some(&s.bob.0)).await;
    assert!(after.body.contains(r#"<span class="chip">Expired</span>"#));
    assert!(!after.body.contains("Cancel"));
}

#[tokio::test]
async fn nobody_can_cancel_someone_elses_subscription() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    let body = format!("product=1&guild={BOB_GUILD}&csrf={}", s.alice.1);
    let r = s
        .app
        .post("/subscriptions/cancel", Some(&s.alice.0), &body)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(status(&s), Some(PremiumStatus::Active));
}

#[tokio::test]
async fn canceling_needs_a_subscription_for_that_exact_server_and_bot() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    let second = format!("name=Second+Bot&csrf={}", s.alice.1);
    assert_eq!(
        s.app
            .post("/products", Some(&s.alice.0), &second)
            .await
            .location(),
        Some("/products/2")
    );
    let other_guild = "2000000000000000003";
    for (product, guild) in [(1, other_guild), (2, BOB_GUILD)] {
        let body = format!("product={product}&guild={guild}&csrf={}", s.bob.1);
        let r = s
            .app
            .post("/subscriptions/cancel", Some(&s.bob.0), &body)
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{product} {guild}");
    }
    assert_eq!(status(&s), Some(PremiumStatus::Active));
    assert!(s.app.entitlement(2_000_000_000_000_000_003, 1).is_none());
}

#[tokio::test]
async fn cancel_checks_the_token_and_the_identifier() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    let wrong = s
        .app
        .post(
            "/subscriptions/cancel",
            Some(&s.bob.0),
            &format!("product=1&guild={BOB_GUILD}&csrf=wrong"),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let bad = s
        .app
        .post(
            "/subscriptions/cancel",
            Some(&s.bob.0),
            &format!("product=1&guild=abc&csrf={}", s.bob.1),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(status(&s), Some(PremiumStatus::Active));
}

#[tokio::test]
async fn a_new_subscriber_for_the_same_server_takes_over() {
    let s = scene().await;
    subscribe(&s, BOB_GUILD, &s.bob.1).await;
    let stored = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.subscriptions_of(1_000_000_000_000_000_002).ok());
    assert_eq!(stored.map(|v| v.len()), Some(1));
}

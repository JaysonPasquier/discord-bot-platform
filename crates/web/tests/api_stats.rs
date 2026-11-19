mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::Plan;
use serde_json::{json, Value};
use support::*;

const DAY: i64 = 86_400;
const ALICE: u64 = 1_000_000_000_000_000_001;
const BOB: u64 = 1_000_000_000_000_000_002;

fn body(resp: &Resp) -> Value {
    serde_json::from_str(&resp.body).unwrap_or_default()
}

struct Scene {
    app: App,
    alice: (String, String),
    bob: (String, String),
}

impl Scene {
    async fn stats(&self, who: &(String, String)) -> Resp {
        self.app
            .api("GET", "/api/bots/1/stats", Some(&who.0), None, None)
            .await
    }
    async fn post(&self, who: &(String, String), path: &str, value: Value) -> Resp {
        self.app
            .api(
                "POST",
                path,
                Some(&who.0),
                Some(&who.1),
                Some(&value.to_string()),
            )
            .await
    }
    async fn guild(&self, who: &(String, String)) -> String {
        body(
            &self
                .app
                .api("GET", "/api/session", Some(&who.0), None, None)
                .await,
        )["guilds"][0]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

async fn scene() -> Scene {
    let app = App::new(true, false);
    let alice = app.dev_login("alice").await;
    let bob = app.dev_login("bob").await;
    app.api(
        "POST",
        "/api/bots",
        Some(&alice.0),
        Some(&alice.1),
        Some(r#"{"name":"Alice Bot"}"#),
    )
    .await;
    app.seed_bot(1, "free", "free.tok", "111");
    app.seed_bot(1, "premium", "premium.tok", "222");
    Scene { app, alice, bob }
}

#[tokio::test]
async fn stats_are_private_to_the_owner() {
    let s = scene().await;
    assert_eq!(s.stats(&s.alice).await.status, StatusCode::OK);
    assert_eq!(s.stats(&s.bob).await.status, StatusCode::NOT_FOUND);
    let anonymous = s
        .app
        .api("GET", "/api/bots/1/stats", None, None, None)
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_new_bot_has_zeros_and_unknowns_never_invented_numbers() {
    let s = scene().await;
    let stats = body(&s.stats(&s.alice).await);
    assert!(stats["servers"]["total"].is_null() && stats["servers"]["free"].is_null());
    assert_eq!(stats["revenue"]["last_30_days_cents"], 0);
    assert_eq!(stats["revenue"]["simulated"], true);
    assert_eq!(stats["subscribers_now"], 0);
    let days = stats["deployments_per_day"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|d| d["count"] == 0));
    let history = stats["subscribers_history"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(history.len(), 30);
    assert_eq!(history.last().map(|p| p["count"].clone()), Some(json!(0)));
    assert!(history[0]["count"].is_null());
}

#[tokio::test]
async fn servers_come_from_discord_per_version_and_are_cached_ten_minutes() {
    let s = scene().await;
    s.app.bots.set_guild_count("free.tok", 12);
    let first = body(&s.stats(&s.alice).await);
    assert_eq!(first["servers"]["free"], 12);
    assert!(first["servers"]["premium"].is_null());
    assert_eq!(first["servers"]["total"], 12);
    let calls = s.app.bots.count_calls();
    assert_eq!(calls, 2);
    s.app.bots.set_guild_count("free.tok", 99);
    s.stats(&s.alice).await;
    assert_eq!(s.app.bots.count_calls(), calls, "still cached");
    s.app.advance(601);
    let later = body(&s.stats(&s.alice).await);
    assert_eq!(later["servers"]["free"], 99);
    assert_eq!(s.app.bots.count_calls(), calls + 2);
}

#[tokio::test]
async fn when_discord_gives_no_number_the_last_known_one_is_kept() {
    let s = scene().await;
    s.app.bots.set_guild_count("free.tok", 5);
    s.app.bots.set_guild_count("premium.tok", 3);
    assert_eq!(body(&s.stats(&s.alice).await)["servers"]["total"], 8);
    s.app
        .bots
        .counts
        .lock()
        .map(|mut c| c.clear())
        .unwrap_or_default();
    s.app.advance(601);
    let again = body(&s.stats(&s.alice).await);
    assert_eq!(
        (
            again["servers"]["free"].clone(),
            again["servers"]["premium"].clone()
        ),
        (json!(5), json!(3))
    );
}

#[tokio::test]
async fn revenue_is_the_sum_of_simulated_payments_of_the_last_30_days_only() {
    let s = scene().await;
    s.post(
        &s.alice,
        "/api/bots/1/listing",
        json!({
            "description": "", "category": "other", "commands": [],
            "plans": [{"name": "Pro", "price_cents": 500, "days": 30}, {"name": "Max", "price_cents": 1500, "days": 30}],
        }),
    )
    .await;
    let page = body(
        &s.app
            .api("GET", "/api/bots/1/listing", Some(&s.alice.0), None, None)
            .await,
    );
    let (pro, max) = (
        page["plans"][0]["id"].clone(),
        page["plans"][1]["id"].clone(),
    );
    let (ag, bg) = (s.guild(&s.alice).await, s.guild(&s.bob).await);
    s.post(
        &s.alice,
        "/api/subscribe",
        json!({"product": 1, "guild": ag, "plan": pro}),
    )
    .await;
    s.post(
        &s.bob,
        "/api/subscribe",
        json!({"product": 1, "guild": bg, "plan": max}),
    )
    .await;
    let at = s.app.now.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        s.app
            .stripe_webhook(&stripe_checkout_completed(
                &ag, 1, ALICE, 30, "Pro", 500, at
            ))
            .await,
        StatusCode::OK
    );
    assert_eq!(
        s.app
            .stripe_webhook(&stripe_checkout_completed(&bg, 1, BOB, 30, "Max", 1500, at))
            .await,
        StatusCode::OK
    );
    let now = body(&s.stats(&s.alice).await);
    assert_eq!(now["revenue"]["last_30_days_cents"], 2000);
    assert_eq!(now["revenue"]["previous_30_days_cents"], 0);
    s.app.advance(31 * DAY);
    // La session a expiré entre-temps : on se reconnecte.
    let alice = s.app.dev_login("alice").await;
    let later = body(&s.stats(&alice).await);
    assert_eq!(later["revenue"]["last_30_days_cents"], 0);
    assert_eq!(later["revenue"]["previous_30_days_cents"], 2000);
}

#[tokio::test]
async fn a_bot_without_any_priced_plan_cannot_be_subscribed_to() {
    let s = scene().await;
    let g = s.guild(&s.alice).await;
    let r = s
        .post(
            &s.alice,
            "/api/subscribe",
            json!({"product": 1, "guild": g}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let stats = body(&s.stats(&s.alice).await);
    assert_eq!(stats["revenue"]["last_30_days_cents"], 0);
    assert_eq!(stats["subscribers_now"], 0);
}

#[tokio::test]
async fn subscribers_over_time_follow_subscriptions_and_cancellation() {
    let s = scene().await;
    assert!(s.app.store.lock().is_ok_and(|store| store
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
    let (ag, bg) = (s.guild(&s.alice).await, s.guild(&s.bob).await);
    s.post(
        &s.alice,
        "/api/subscribe",
        json!({"product": 1, "guild": ag}),
    )
    .await;
    assert_eq!(
        s.app
            .stripe_webhook(&stripe_checkout_completed(
                &ag,
                1,
                ALICE,
                30,
                "Pro",
                500,
                s.app.now.load(std::sync::atomic::Ordering::SeqCst)
            ))
            .await,
        StatusCode::OK
    );
    s.app.advance(DAY);
    s.post(&s.bob, "/api/subscribe", json!({"product": 1, "guild": bg}))
        .await;
    assert_eq!(
        s.app
            .stripe_webhook(&stripe_checkout_completed(
                &bg,
                1,
                BOB,
                30,
                "Pro",
                500,
                s.app.now.load(std::sync::atomic::Ordering::SeqCst)
            ))
            .await,
        StatusCode::OK
    );
    s.app.advance(DAY);
    s.post(
        &s.alice,
        "/api/subscriptions/cancel",
        json!({"product": 1, "guild": ag}),
    )
    .await;
    let stats = body(&s.stats(&s.alice).await);
    let counts: Vec<Value> = stats["subscribers_history"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .rev()
        .take(3)
        .map(|p| p["count"].clone())
        .collect();
    // Aujourd'hui : bob seul. Hier : les deux. Avant-hier : alice seule.
    assert_eq!(counts, [json!(1), json!(2), json!(1)]);
    assert_eq!(stats["subscribers_now"], 1);
}

#[tokio::test]
async fn deployments_are_counted_per_utc_day_over_the_last_seven_days() {
    let s = scene().await;
    s.app.github.offer(7, "alice/bot", &[("main", HEAD_A)]);
    let installation = platform_store::StoredInstallation {
        id: 7,
        login: "alice".into(),
        selection: "selected".into(),
    };
    s.app
        .store
        .lock()
        .map(|st| st.replace_installations(1_000_000_000_000_000_001, &[installation]))
        .ok();
    let source =
        json!({"installation": 7, "repo": "alice/bot", "branch": "main", "auto_deploy": false});
    s.post(&s.alice, "/api/bots/1/source/free", source).await;
    for _ in 0..2 {
        s.post(
            &s.alice,
            "/api/bots/1/deployments",
            json!({"tier": "free", "branch": "main"}),
        )
        .await;
    }
    s.app.advance(DAY);
    s.post(
        &s.alice,
        "/api/bots/1/deployments",
        json!({"tier": "free", "branch": "main"}),
    )
    .await;
    let days = body(&s.stats(&s.alice).await)["deployments_per_day"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let counts: Vec<u64> = days
        .iter()
        .map(|d| d["count"].as_u64().unwrap_or(99))
        .collect();
    assert_eq!(counts.iter().sum::<u64>(), 3);
    assert_eq!((counts[5], counts[6]), (2, 1));
    s.app.advance(7 * DAY);
    let later = body(&s.stats(&s.alice).await)["deployments_per_day"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(later.iter().all(|d| d["count"] == 0));
}

#[tokio::test]
async fn the_public_page_shows_the_last_known_server_count_or_nothing() {
    let s = scene().await;
    let before = body(&s.app.api("GET", "/api/market/1", None, None, None).await);
    assert!(before["servers"].is_null());
    s.app.bots.set_guild_count("free.tok", 7);
    s.stats(&s.alice).await;
    let after = body(&s.app.api("GET", "/api/market/1", None, None, None).await);
    assert_eq!(after["servers"], 7);
}

#[tokio::test]
async fn deleting_a_bot_removes_its_stats_data() {
    let s = scene().await;
    s.app.bots.set_guild_count("free.tok", 4);
    let g = s.guild(&s.alice).await;
    s.post(
        &s.alice,
        "/api/subscribe",
        json!({"product": 1, "guild": g}),
    )
    .await;
    s.stats(&s.alice).await;
    s.post(
        &s.alice,
        "/api/bots/1/delete",
        json!({"confirm": "Alice Bot"}),
    )
    .await;
    let left = s.app.store.lock().map(|st| {
        let p = platform_core::ProductId(1);
        (
            st.guild_counts(p).map(|c| c.len()).unwrap_or(9),
            st.snapshots_since(p, 0).map(|c| c.len()).unwrap_or(9),
            st.revenue_between(p, 0, i64::MAX).unwrap_or(9),
        )
    });
    assert_eq!(left.ok(), Some((0, 0, 0)));
}

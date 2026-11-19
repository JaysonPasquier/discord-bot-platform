mod support;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_store::{Plan, StoredInstallation};
use serde_json::Value;
use support::*;

const ALICE: u64 = 1_000_000_000_000_000_001;
const REPO: &str = "alice/bot";

fn json(resp: &Resp) -> Value {
    serde_json::from_str(&resp.body).unwrap_or_default()
}

fn code(resp: &Resp) -> String {
    json(resp)["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

struct Scene {
    app: App,
    alice: (String, String),
    bob: (String, String),
}

impl Scene {
    async fn post(&self, who: &(String, String), path: &str, body: &str) -> Resp {
        self.app
            .api("POST", path, Some(&who.0), Some(&who.1), Some(body))
            .await
    }
    async fn get(&self, who: &(String, String), path: &str) -> Resp {
        self.app.api("GET", path, Some(&who.0), None, None).await
    }
}

// Alice a le bot 1 (deux jetons), une installation GitHub (7) et un dépôt à deux branches. Bob existe aussi.
async fn scene() -> Scene {
    scene_with(0).await
}

async fn scene_with(cooldown: u32) -> Scene {
    let app = App::with_limits(true, false, 10, cooldown);
    let alice = app.dev_login("alice").await;
    let bob = app.dev_login("bob").await;
    let made = app
        .api(
            "POST",
            "/api/bots",
            Some(&alice.0),
            Some(&alice.1),
            Some(r#"{"name":"Alice Bot"}"#),
        )
        .await;
    assert_eq!(made.status, StatusCode::CREATED);
    app.seed_bot(1, "free", "free.tok", "111");
    app.seed_bot(1, "premium", "premium.tok", "222");
    app.github
        .offer(7, REPO, &[("main", HEAD_A), ("dev", HEAD_B)]);
    let stored = app.store.lock().is_ok_and(|s| {
        let i = StoredInstallation {
            id: 7,
            login: "alice".into(),
            selection: "selected".into(),
        };
        s.replace_installations(ALICE, &[i]).is_ok()
    });
    assert!(stored);
    Scene { app, alice, bob }
}

#[tokio::test]
async fn rename_checks_the_rule_and_the_owner() {
    let s = scene().await;
    let bad = s
        .post(&s.alice, "/api/bots/1/rename", r#"{"name":"x"}"#)
        .await;
    assert_eq!(
        (bad.status, code(&bad).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_name")
    );
    let foreign = s
        .post(&s.bob, "/api/bots/1/rename", r#"{"name":"Stolen Bot"}"#)
        .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    let ok = s
        .post(&s.alice, "/api/bots/1/rename", r#"{"name":"Renamed Bot"}"#)
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(
        json(&s.get(&s.alice, "/api/bots/1").await)["name"],
        "Renamed Bot"
    );
}

#[tokio::test]
async fn stop_targets_one_version_or_both_and_rejects_strangers() {
    let s = scene().await;
    assert_eq!(
        s.post(&s.bob, "/api/bots/1/stop", "{}").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.post(&s.alice, "/api/bots/1/stop", r#"{"tier":"gold"}"#)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    s.post(&s.alice, "/api/bots/1/stop", r#"{"tier":"free"}"#)
        .await;
    assert_eq!(
        s.app
            .containers
            .stopped
            .lock()
            .map(|l| l.len())
            .unwrap_or(0),
        1
    );
    s.post(&s.alice, "/api/bots/1/stop", "{}").await;
    assert_eq!(
        s.app
            .containers
            .stopped
            .lock()
            .map(|l| l.len())
            .unwrap_or(0),
        3
    );
}

#[tokio::test]
async fn delete_needs_the_exact_name_and_stops_the_containers() {
    let s = scene().await;
    let wrong = s
        .post(&s.alice, "/api/bots/1/delete", r#"{"confirm":"alice bot"}"#)
        .await;
    assert_eq!(
        (wrong.status, code(&wrong).as_str()),
        (StatusCode::BAD_REQUEST, "name_mismatch")
    );
    assert_eq!(
        s.post(&s.bob, "/api/bots/1/delete", r#"{"confirm":"Alice Bot"}"#)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(s.get(&s.alice, "/api/bots/1").await.status, StatusCode::OK);
    let ok = s
        .post(&s.alice, "/api/bots/1/delete", r#"{"confirm":"Alice Bot"}"#)
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(
        s.get(&s.alice, "/api/bots/1").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .containers
            .stopped
            .lock()
            .map(|l| l.len())
            .unwrap_or(0),
        2
    );
}

#[tokio::test]
async fn a_token_is_checked_with_discord_stored_sealed_and_never_returned() {
    let s = scene().await;
    s.app.bots.know("new.tok", "333", "Brand New");
    let bad = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"free","token":"has space"}"#,
        )
        .await;
    assert_eq!(code(&bad), "invalid_token");
    let unknown = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"free","token":"nobody.tok"}"#,
        )
        .await;
    assert_eq!(code(&unknown), "token_refused");
    let ok = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"premium","token":"new.tok"}"#,
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(json(&ok)["client_id"], "333");
    assert!(!ok.body.contains("new.tok"));
    assert_eq!(
        s.app
            .stored_credential(1, "premium")
            .map(|c| c.client_id)
            .as_deref(),
        Some("333")
    );
}

#[tokio::test]
async fn checking_tokens_and_deploying_are_rate_limited_per_version() {
    let s = scene_with(30).await;
    s.app.bots.know("a.tok", "444", "A");
    let first = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"free","token":"a.tok"}"#,
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let again = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"free","token":"a.tok"}"#,
        )
        .await;
    assert_eq!(
        (again.status, code(&again).as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "slow_down")
    );
    // L'autre version a son propre compteur.
    s.app.bots.know("b.tok", "555", "B");
    let other = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"premium","token":"b.tok"}"#,
        )
        .await;
    assert_eq!(other.status, StatusCode::OK);
    s.post(&s.alice, "/api/bots/1/source/free", &source_body(""))
        .await;
    let one = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"free","branch":"main"}"#,
        )
        .await;
    assert_eq!(one.status, StatusCode::ACCEPTED);
    let two = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"free","branch":"main"}"#,
        )
        .await;
    assert_eq!(two.status, StatusCode::TOO_MANY_REQUESTS);
    s.app.advance(31);
    let three = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"free","branch":"main"}"#,
        )
        .await;
    assert_eq!(three.status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn a_token_already_used_by_another_version_is_a_conflict() {
    let s = scene().await;
    s.app.bots.know("dup.tok", "222", "Duplicate");
    let r = s
        .post(
            &s.alice,
            "/api/bots/1/token",
            r#"{"tier":"free","token":"dup.tok"}"#,
        )
        .await;
    assert_eq!(
        (r.status, code(&r).as_str()),
        (StatusCode::CONFLICT, "app_in_use")
    );
}

#[tokio::test]
async fn github_lists_only_my_accounts_repositories_and_branches() {
    let s = scene().await;
    let state = json(&s.get(&s.alice, "/api/github").await);
    assert_eq!(state["configured"], true);
    // L'adresse du serveur (jamais celle du frontend, ils ne partagent pas forcément l'origine).
    assert_eq!(state["webhook_url"], "http://127.0.0.1:8080/github/webhook");
    assert_eq!(state["installations"][0]["id"], "7");
    let bob_state = json(&s.get(&s.bob, "/api/github").await);
    assert_eq!(bob_state["installations"].as_array().map(Vec::len), Some(0));
    let repos = s.get(&s.alice, "/api/github/installations/7/repos").await;
    assert_eq!(json(&repos)[0]["full_name"], REPO);
    assert_eq!(
        s.get(&s.bob, "/api/github/installations/7/repos")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let branches = s
        .get(
            &s.alice,
            &format!(
                "/api/github/branches?installation=7&repo={}",
                form_encode(REPO)
            ),
        )
        .await;
    assert_eq!(json(&branches).as_array().map(Vec::len), Some(2));
    let other = s
        .get(
            &s.bob,
            &format!(
                "/api/github/branches?installation=7&repo={}",
                form_encode(REPO)
            ),
        )
        .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
}

fn source_body(extra: &str) -> String {
    format!(r#"{{"installation":7,"repo":"{REPO}","branch":"main","auto_deploy":true{extra}}}"#)
}

#[tokio::test]
async fn connecting_a_source_rechecks_github_and_can_serve_both_versions() {
    let s = scene().await;
    for forged in [
        r#"{"installation":99,"repo":"alice/bot","branch":"main","auto_deploy":true}"#,
        r#"{"installation":7,"repo":"alice/secret","branch":"main","auto_deploy":true}"#,
        r#"{"installation":7,"repo":"alice/bot","branch":"nope","auto_deploy":true}"#,
        r#"{"installation":7,"repo":"..","branch":"main","auto_deploy":true}"#,
    ] {
        let r = s.post(&s.alice, "/api/bots/1/source/free", forged).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{forged}");
    }
    assert_eq!(
        s.post(&s.bob, "/api/bots/1/source/free", &source_body(""))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let ok = s
        .post(
            &s.alice,
            "/api/bots/1/source/free",
            &source_body(r#","both":true"#),
        )
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    let bot = json(&s.get(&s.alice, "/api/bots/1").await);
    assert_eq!(bot["versions"][0]["source"]["repo"], REPO);
    assert_eq!(bot["versions"][1]["source"]["repo"], REPO);
}

#[tokio::test]
async fn auto_deploy_can_be_toggled_and_a_source_removed() {
    let s = scene().await;
    s.post(&s.alice, "/api/bots/1/source/free", &source_body(""))
        .await;
    s.post(
        &s.alice,
        "/api/bots/1/source/free/auto",
        r#"{"auto_deploy":false}"#,
    )
    .await;
    let off = json(&s.get(&s.alice, "/api/bots/1").await);
    assert_eq!(off["versions"][0]["source"]["auto_deploy"], false);
    assert_eq!(
        s.post(
            &s.alice,
            "/api/bots/1/source/premium/auto",
            r#"{"auto_deploy":true}"#
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.post(&s.alice, "/api/bots/1/source/free/remove", "{}")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(json(&s.get(&s.alice, "/api/bots/1").await)["versions"][0]["source"].is_null());
}

#[tokio::test]
async fn deploying_a_branch_queues_a_build_of_its_real_head_and_redeploy_reuses_the_commit() {
    let s = scene().await;
    s.post(&s.alice, "/api/bots/1/source/free", &source_body(""))
        .await;
    let none = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"premium","branch":"main"}"#,
        )
        .await;
    assert_eq!(
        (none.status, code(&none).as_str()),
        (StatusCode::CONFLICT, "no_source")
    );
    let unknown = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"free","branch":"nope"}"#,
        )
        .await;
    assert_eq!(code(&unknown), "branch_unknown");
    let ok = s
        .post(
            &s.alice,
            "/api/bots/1/deployments",
            r#"{"tier":"free","branch":"dev"}"#,
        )
        .await;
    assert_eq!(ok.status, StatusCode::ACCEPTED);
    let id = json(&ok)["id"].as_u64().unwrap_or_default();
    let view = json(
        &s.get(&s.alice, &format!("/api/bots/1/deployments/{id}"))
            .await,
    );
    assert_eq!(view["deployment"]["commit"], HEAD_B);
    assert_eq!(view["deployment"]["status"], "running");
    assert!(view["deployment"]["log"]
        .as_str()
        .unwrap_or_default()
        .contains("Live."));
    assert_eq!(
        s.get(&s.bob, &format!("/api/bots/1/deployments/{id}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let again = s
        .post(
            &s.alice,
            &format!("/api/bots/1/deployments/{id}/redeploy"),
            "{}",
        )
        .await;
    assert_eq!(again.status, StatusCode::ACCEPTED);
    assert_ne!(json(&again)["id"].as_u64(), Some(id));
    assert_eq!(
        s.get(&s.alice, "/api/bots/1/deployments/999").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_marketplace_is_public_and_lists_only_connected_bots() {
    let s = scene().await;
    s.post(&s.alice, "/api/bots", r#"{"name":"Hidden Bot"}"#)
        .await;
    let r = s.app.api("GET", "/api/market", None, None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let bots = json(&r)["bots"].as_array().cloned().unwrap_or_default();
    assert_eq!(bots.len(), 1);
    assert_eq!(bots[0]["name"], "Alice Bot");
    assert_eq!(
        (
            bots[0]["has_free"].as_bool(),
            bots[0]["has_premium"].as_bool()
        ),
        (Some(true), Some(true))
    );
    assert!(!r.body.contains("free.tok"));
}

#[tokio::test]
async fn a_listing_hides_an_unconnected_bot_from_everyone_but_its_owner() {
    let s = scene().await;
    s.post(&s.alice, "/api/bots", r#"{"name":"Hidden Bot"}"#)
        .await;
    assert_eq!(
        s.app
            .api("GET", "/api/market/2", None, None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get(&s.bob, "/api/market/2").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.get(&s.alice, "/api/market/2").await.status,
        StatusCode::OK
    );
    let public = json(&s.app.api("GET", "/api/market/1", None, None, None).await);
    assert_eq!(public["signed_in"], false);
    assert!(public["free_invite"]
        .as_str()
        .unwrap_or_default()
        .contains("111"));
    assert_eq!(public["guilds"].as_array().map(Vec::len), Some(0));
    let bob = json(&s.get(&s.bob, "/api/market/1").await);
    assert_eq!(bob["guilds"][0]["name"], "Bob Dev Server A");
}

#[tokio::test]
async fn subscribing_needs_a_server_the_user_manages_and_cancelling_needs_the_subscription() {
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
    let bob_guild = json(&s.get(&s.bob, "/api/session").await)["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let body = |g: &str| format!(r#"{{"product":1,"guild":"{g}"}}"#);
    let alice_guild = json(&s.get(&s.alice, "/api/session").await)["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let foreign = s.post(&s.alice, "/api/subscribe", &body(&bob_guild)).await;
    assert_eq!(
        (foreign.status, code(&foreign).as_str()),
        (StatusCode::FORBIDDEN, "not_your_server")
    );
    assert_eq!(
        code(&s.post(&s.alice, "/api/subscribe", &body("12x")).await),
        "invalid_server"
    );
    assert_eq!(
        s.post(&s.alice, "/api/subscribe", r#"{"product":99,"guild":"1"}"#)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Créer la session Checkout n'accorde rien tant que Stripe n'a pas confirmé le paiement.
    let checkout = s
        .post(&s.alice, "/api/subscribe", &body(&alice_guild))
        .await;
    assert_eq!(checkout.status, StatusCode::OK);
    assert!(json(&checkout)["checkout_url"]
        .as_str()
        .unwrap_or_default()
        .starts_with("https://"));
    assert_eq!(
        json(&s.get(&s.alice, "/api/subscriptions").await)["subscriptions"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // Le webhook Stripe applique le paiement, comme un vrai `checkout.session.completed`.
    let body = stripe_checkout_completed(
        &alice_guild,
        1,
        ALICE,
        30,
        "Pro",
        500,
        s.app.now.load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(s.app.stripe_webhook(&body).await, StatusCode::OK);
    let list = json(&s.get(&s.alice, "/api/subscriptions").await);
    assert_eq!(list["subscriptions"][0]["status"], "active");
    assert_eq!(list["subscriptions"][0]["bot"], "Alice Bot");
    assert!(!s
        .get(&s.bob, "/api/subscriptions")
        .await
        .body
        .contains("Alice Bot"));
    // Rejouer le même événement ne double rien (idempotence).
    assert_eq!(s.app.stripe_webhook(&body).await, StatusCode::OK);
    assert_eq!(
        s.post(
            &s.bob,
            "/api/subscriptions/cancel",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#)
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.post(
            &s.alice,
            "/api/subscriptions/cancel",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#)
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    let after = json(&s.get(&s.alice, "/api/subscriptions").await);
    assert_eq!(after["subscriptions"][0]["canceled"], true);
}

#[tokio::test]
async fn subscribing_without_a_priced_plan_is_refused() {
    let s = scene().await;
    let alice_guild = json(&s.get(&s.alice, "/api/session").await)["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let no_plan = s
        .post(
            &s.alice,
            "/api/subscribe",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#),
        )
        .await;
    assert_eq!(
        (no_plan.status, code(&no_plan).as_str()),
        (StatusCode::CONFLICT, "no_plan")
    );
    assert!(s.app.store.lock().is_ok_and(|store| store
        .replace_plans(
            ProductId(1),
            &[Plan {
                id: 0,
                name: "Free trial".to_string(),
                price_cents: 0,
                days: 30
            }]
        )
        .is_ok()));
    let zero = s
        .post(
            &s.alice,
            "/api/subscribe",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#),
        )
        .await;
    assert_eq!(
        (zero.status, code(&zero).as_str()),
        (StatusCode::CONFLICT, "free_plan")
    );
}

#[tokio::test]
async fn an_async_payment_method_only_grants_access_once_actually_paid() {
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
    let alice_guild = json(&s.get(&s.alice, "/api/session").await)["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        s.post(
            &s.alice,
            "/api/subscribe",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#)
        )
        .await
        .status,
        StatusCode::OK
    );
    let at = s.app.now.load(std::sync::atomic::Ordering::SeqCst);
    // La session se termine d'abord sans être payée (virement en attente) : aucun accès encore.
    let unpaid = stripe_checkout_unpaid(&alice_guild, 1, ALICE, 30, "Pro", 500, at);
    assert_eq!(s.app.stripe_webhook(&unpaid).await, StatusCode::OK);
    assert_eq!(
        json(&s.get(&s.alice, "/api/subscriptions").await)["subscriptions"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // Le virement arrive : l'accès est accordé à ce moment-là seulement.
    let succeeded = stripe_async_payment_succeeded(&alice_guild, 1, ALICE, 30, "Pro", 500, at);
    assert_eq!(s.app.stripe_webhook(&succeeded).await, StatusCode::OK);
    let list = json(&s.get(&s.alice, "/api/subscriptions").await);
    assert_eq!(list["subscriptions"][0]["status"], "active");
}

// Un litige ne porte que l'identifiant du PaymentIntent (`stripe_intents`, rempli au paiement) :
// `created` révoque aussitôt, `closed` ne lève la révocation que si le litige est gagné.
#[tokio::test]
async fn a_dispute_revokes_access_and_only_winning_it_restores_access() {
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
    let alice_guild = json(&s.get(&s.alice, "/api/session").await)["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        s.post(
            &s.alice,
            "/api/subscribe",
            &format!(r#"{{"product":1,"guild":"{alice_guild}"}}"#)
        )
        .await
        .status,
        StatusCode::OK
    );
    let at = s.app.now.load(std::sync::atomic::Ordering::SeqCst);
    let completed = stripe_checkout_completed(&alice_guild, 1, ALICE, 30, "Pro", 500, at);
    assert_eq!(s.app.stripe_webhook(&completed).await, StatusCode::OK);
    let guild_num: u64 = alice_guild.parse().unwrap_or(0);
    assert_eq!(
        s.app.entitlement(guild_num, 1).map(|e| e.revoked),
        Some(false)
    );
    let payment_intent = format!("pi_{alice_guild}_1_{at}");

    let created = stripe_dispute_event(
        "charge.dispute.created",
        &payment_intent,
        "needs_response",
        "1",
    );
    assert_eq!(s.app.stripe_webhook(&created).await, StatusCode::OK);
    assert_eq!(
        s.app.entitlement(guild_num, 1).map(|e| e.revoked),
        Some(true)
    );

    // Perdu (ou tout statut différent de "won") : la révocation reste en place.
    let lost = stripe_dispute_event("charge.dispute.closed", &payment_intent, "lost", "2");
    assert_eq!(s.app.stripe_webhook(&lost).await, StatusCode::OK);
    assert_eq!(
        s.app.entitlement(guild_num, 1).map(|e| e.revoked),
        Some(true)
    );

    let won = stripe_dispute_event("charge.dispute.closed", &payment_intent, "won", "3");
    assert_eq!(s.app.stripe_webhook(&won).await, StatusCode::OK);
    assert_eq!(
        s.app.entitlement(guild_num, 1).map(|e| e.revoked),
        Some(false)
    );
}

#[tokio::test]
async fn a_dispute_for_an_unknown_payment_intent_is_a_harmless_no_op() {
    let s = scene().await;
    let unknown = stripe_dispute_event(
        "charge.dispute.created",
        "pi_never_seen",
        "needs_response",
        "1",
    );
    assert_eq!(s.app.stripe_webhook(&unknown).await, StatusCode::OK);
}

#[tokio::test]
async fn an_unsigned_stripe_webhook_is_refused() {
    let s = scene().await;
    let body = stripe_checkout_completed(
        "500",
        1,
        ALICE,
        30,
        "Pro",
        500,
        s.app.now.load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(
        s.app.stripe_webhook_unsigned(&body).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(s.app.stripe_webhook(&body).await, StatusCode::OK);
}

#[tokio::test]
async fn the_docs_are_public_and_include_the_sdk() {
    let s = scene().await;
    let r = s.app.api("GET", "/api/docs", None, None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let docs = json(&r);
    assert_eq!(docs["pages"][0]["slug"], "quickstart");
    assert!(docs["sdk"]
        .as_str()
        .unwrap_or_default()
        .contains("isPremium"));
}

#[tokio::test]
async fn every_new_write_endpoint_refuses_a_missing_form_token_and_a_missing_session() {
    let s = scene().await;
    for path in [
        "rename",
        "stop",
        "delete",
        "token",
        "run",
        "deployments",
        "source/free",
        "source/free/auto",
        "source/free/remove",
    ] {
        let url = format!("/api/bots/1/{path}");
        let anonymous = s.app.api("POST", &url, None, None, Some("{}")).await;
        assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED, "{path}");
        let no_csrf = s
            .app
            .api("POST", &url, Some(&s.alice.0), None, Some("{}"))
            .await;
        assert_eq!(no_csrf.status, StatusCode::FORBIDDEN, "{path}");
    }
    for path in ["/api/subscribe", "/api/subscriptions/cancel"] {
        let no_csrf = s
            .app
            .api("POST", path, Some(&s.alice.0), None, Some("{}"))
            .await;
        assert_eq!(no_csrf.status, StatusCode::FORBIDDEN, "{path}");
    }
}

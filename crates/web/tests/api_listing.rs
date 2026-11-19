mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use support::*;

const PNG: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];

fn body(resp: &Resp) -> Value {
    serde_json::from_str(&resp.body).unwrap_or_default()
}

fn code(resp: &Resp) -> String {
    body(resp)["error"]["code"]
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
    async fn save(&self, who: &(String, String), value: Value) -> Resp {
        let text = value.to_string();
        self.app
            .api(
                "POST",
                "/api/bots/1/listing",
                Some(&who.0),
                Some(&who.1),
                Some(&text),
            )
            .await
    }
}

// Alice possède le bot 1 ; seule sa version gratuite est connectée.
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
    Scene { app, alice, bob }
}

fn good() -> Value {
    json!({
        "description": "Keeps your server tidy.\nFree and fast.",
        "category": "moderation",
        "commands": [{"name": "/ban", "description": "Ban a member"}, {"name": "warn", "description": "Warn a member"}],
        "plans": [{"name": "Pro", "price_cents": 500, "days": 30}, {"name": "Year", "price_cents": 4800, "days": 365}],
    })
}

#[tokio::test]
async fn a_new_bot_has_an_empty_page_and_only_its_owner_reads_the_editor() {
    let s = scene().await;
    let r = s
        .app
        .api("GET", "/api/bots/1/listing", Some(&s.alice.0), None, None)
        .await;
    let page = body(&r);
    assert_eq!(
        (page["description"].as_str(), page["category"].as_str()),
        (Some(""), Some("other"))
    );
    assert_eq!(page["commands"].as_array().map(Vec::len), Some(0));
    assert_eq!(page["categories"].as_array().map(Vec::len), Some(8));
    assert_eq!(
        s.app
            .api("GET", "/api/bots/1/listing", Some(&s.bob.0), None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .api("GET", "/api/bots/1/listing", None, None, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn saving_stores_text_commands_and_plans_and_returns_them_cleaned() {
    let s = scene().await;
    let r = s.save(&s.alice, good()).await;
    assert_eq!(r.status, StatusCode::OK);
    let page = body(&r);
    assert_eq!(page["category"], "moderation");
    assert_eq!(page["commands"][0]["name"], "ban");
    assert_eq!(page["plans"][1]["days"], 365);
    assert!(page["plans"][0]["id"].as_u64().unwrap_or(0) > 0);
    let again = body(
        &s.app
            .api("GET", "/api/bots/1/listing", Some(&s.alice.0), None, None)
            .await,
    );
    assert_eq!(
        again["description"],
        "Keeps your server tidy.\nFree and fast."
    );
}

#[tokio::test]
async fn every_invalid_field_is_refused_with_its_own_code_and_nothing_is_saved() {
    let s = scene().await;
    let cases = [
        ("description", json!("x".repeat(501)), "invalid_description"),
        ("category", json!("weapons"), "invalid_category"),
        (
            "commands",
            json!([{"name": "Bad Name", "description": "d"}]),
            "invalid_command",
        ),
        (
            "commands",
            json!([{"name": "a", "description": "d"}, {"name": "a", "description": "e"}]),
            "duplicate_command",
        ),
        (
            "plans",
            json!([{"name": "", "price_cents": 1, "days": 30}]),
            "invalid_plan",
        ),
        (
            "plans",
            json!([{"name": "P", "price_cents": 1, "days": 0}]),
            "invalid_plan",
        ),
    ];
    for (field, value, expected) in cases {
        let mut form = good();
        form[field] = value;
        let r = s.save(&s.alice, form).await;
        assert_eq!(
            (r.status, code(&r)),
            (StatusCode::BAD_REQUEST, expected.to_string()),
            "{field}"
        );
    }
    let page = body(
        &s.app
            .api("GET", "/api/bots/1/listing", Some(&s.alice.0), None, None)
            .await,
    );
    assert_eq!(page["description"], "");
    assert_eq!(s.save(&s.bob, good()).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn images_are_checked_by_content_size_and_owner_then_served_publicly() {
    let s = scene().await;
    let up = |who: &(String, String), kind: &str, bytes: Vec<u8>| {
        let (cookie, csrf, path) = (
            who.0.clone(),
            who.1.clone(),
            format!("/api/bots/1/listing/image/{kind}"),
        );
        let app = &s.app;
        async move { app.api_bytes(&path, &cookie, &csrf, bytes).await }
    };
    assert_eq!(
        code(&up(&s.alice, "avatar", b"<svg/>".to_vec()).await),
        "image_type"
    );
    assert_eq!(code(&up(&s.alice, "avatar", vec![]).await), "empty_image");
    let mut big = PNG.to_vec();
    big.resize(2 * 1024 * 1024 + 1, 0);
    let too_big = up(&s.alice, "banner", big).await;
    assert_eq!(
        (too_big.status, code(&too_big)),
        (StatusCode::PAYLOAD_TOO_LARGE, "image_too_large".to_string())
    );
    assert_eq!(
        up(&s.alice, "poster", PNG.to_vec()).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        up(&s.bob, "avatar", PNG.to_vec()).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        up(&s.alice, "avatar", PNG.to_vec()).await.status,
        StatusCode::NO_CONTENT
    );
    let public = s
        .app
        .api("GET", "/api/market/1/image/avatar", None, None, None)
        .await;
    assert_eq!(public.status, StatusCode::OK);
    assert_eq!(
        public
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        s.app
            .api("GET", "/api/market/1/image/banner", None, None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let page = body(
        &s.app
            .api("GET", "/api/bots/1/listing", Some(&s.alice.0), None, None)
            .await,
    );
    assert_eq!(
        (page["has_avatar"].as_bool(), page["has_banner"].as_bool()),
        (Some(true), Some(false))
    );
    let removed = s
        .app
        .api(
            "POST",
            "/api/bots/1/listing/image/avatar/remove",
            Some(&s.alice.0),
            Some(&s.alice.1),
            Some("{}"),
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    assert_eq!(
        s.app
            .api("GET", "/api/market/1/image/avatar", None, None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_image_of_an_unpublished_bot_is_only_for_its_owner() {
    let s = scene().await;
    s.app
        .api(
            "POST",
            "/api/bots",
            Some(&s.alice.0),
            Some(&s.alice.1),
            Some(r#"{"name":"Draft Bot"}"#),
        )
        .await;
    let sent = s
        .app
        .api_bytes(
            "/api/bots/2/listing/image/avatar",
            &s.alice.0,
            &s.alice.1,
            PNG.to_vec(),
        )
        .await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT);
    assert_eq!(
        s.app
            .api("GET", "/api/market/2/image/avatar", None, None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .api(
                "GET",
                "/api/market/2/image/avatar",
                Some(&s.bob.0),
                None,
                None
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .api(
                "GET",
                "/api/market/2/image/avatar",
                Some(&s.alice.0),
                None,
                None
            )
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_public_pages_show_what_the_creator_wrote() {
    let s = scene().await;
    s.save(&s.alice, good()).await;
    let market = body(&s.app.api("GET", "/api/market", None, None, None).await);
    assert_eq!(
        market["bots"][0]["description"],
        "Keeps your server tidy.\nFree and fast."
    );
    assert_eq!(market["bots"][0]["category"], "moderation");
    let page = body(&s.app.api("GET", "/api/market/1", None, None, None).await);
    assert_eq!(page["commands"][1]["name"], "warn");
    assert_eq!(page["plans"][0]["price_cents"], 500);
}

#[tokio::test]
async fn subscribing_uses_the_length_of_the_chosen_plan() {
    let s = scene().await;
    s.app.seed_bot(1, "premium", "premium.tok", "222");
    let saved = body(&s.save(&s.alice, good()).await);
    let year = saved["plans"][1]["id"].as_u64().unwrap_or(0);
    let guild = body(
        &s.app
            .api("GET", "/api/session", Some(&s.alice.0), None, None)
            .await,
    )["guilds"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let call = |plan: Value| json!({"product": 1, "guild": guild, "plan": plan}).to_string();
    let wrong = s
        .app
        .api(
            "POST",
            "/api/subscribe",
            Some(&s.alice.0),
            Some(&s.alice.1),
            Some(&call(json!(9999))),
        )
        .await;
    assert_eq!(code(&wrong), "invalid_plan");
    let ok = s
        .app
        .api(
            "POST",
            "/api/subscribe",
            Some(&s.alice.0),
            Some(&s.alice.1),
            Some(&call(json!(year))),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    const ALICE: u64 = 1_000_000_000_000_000_001;
    let completed = stripe_checkout_completed(
        &guild,
        1,
        ALICE,
        365,
        "Year",
        4800,
        s.app.now.load(std::sync::atomic::Ordering::SeqCst),
    );
    assert_eq!(s.app.stripe_webhook(&completed).await, StatusCode::OK);
    let listed = body(
        &s.app
            .api("GET", "/api/subscriptions", Some(&s.alice.0), None, None)
            .await,
    );
    let until = listed["subscriptions"][0]["paid_until"]
        .as_i64()
        .unwrap_or(0);
    let now = listed["now"].as_i64().unwrap_or(0);
    assert_eq!(until - now, 365 * 86_400);
}

#[tokio::test]
async fn deleting_a_bot_removes_its_page_data() {
    let s = scene().await;
    s.save(&s.alice, good()).await;
    s.app
        .api_bytes(
            "/api/bots/1/listing/image/avatar",
            &s.alice.0,
            &s.alice.1,
            PNG.to_vec(),
        )
        .await;
    let gone = s
        .app
        .api(
            "POST",
            "/api/bots/1/delete",
            Some(&s.alice.0),
            Some(&s.alice.1),
            Some(r#"{"confirm":"Alice Bot"}"#),
        )
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    let left = s.app.store.lock().map(|st| {
        (
            st.listing(platform_core::ProductId(1))
                .ok()
                .flatten()
                .is_some(),
            st.commands_of(platform_core::ProductId(1))
                .map(|c| c.len())
                .unwrap_or(9),
            st.plans_of(platform_core::ProductId(1))
                .map(|c| c.len())
                .unwrap_or(9),
        )
    });
    assert_eq!(left.ok(), Some((false, 0, 0)));
}

#[tokio::test]
async fn the_page_writes_need_a_session_and_the_form_token() {
    let s = scene().await;
    for path in [
        "/api/bots/1/listing",
        "/api/bots/1/listing/image/avatar/remove",
    ] {
        assert_eq!(
            s.app.api("POST", path, None, None, Some("{}")).await.status,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        assert_eq!(
            s.app
                .api("POST", path, Some(&s.alice.0), None, Some("{}"))
                .await
                .status,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let no_csrf = s
        .app
        .api_bytes(
            "/api/bots/1/listing/image/avatar",
            &s.alice.0,
            "wrong",
            PNG.to_vec(),
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
}

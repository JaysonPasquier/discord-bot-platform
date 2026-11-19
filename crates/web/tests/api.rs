mod support;

use axum::http::StatusCode;
use serde_json::Value;
use support::*;

fn json(resp: &Resp) -> Value {
    serde_json::from_str(&resp.body).unwrap_or_default()
}

async fn create(app: &App, cookie: &str, csrf: &str, name: &str) -> Resp {
    app.api(
        "POST",
        "/api/bots",
        Some(cookie),
        Some(csrf),
        Some(&format!(r#"{{"name":"{name}"}}"#)),
    )
    .await
}

#[tokio::test]
async fn a_visitor_gets_a_session_without_user_and_without_token() {
    let app = App::new(true, false);
    let r = app.api("GET", "/api/session", None, None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let body = json(&r);
    assert!(body["user"].is_null() && body["csrf"].is_null());
    assert_eq!(body["login"]["dev"], true);
    assert_eq!(body["login"]["discord"], false);
    assert!(body["now"].as_i64().is_some());
}

#[tokio::test]
async fn dev_login_through_the_api_sets_a_hardened_cookie_and_the_session_names_the_user() {
    let app = App::new(true, false);
    let r = app
        .api(
            "POST",
            "/api/dev-login",
            None,
            None,
            Some(r#"{"who":"alice"}"#),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let cookie = r.set_cookies().into_iter().next().unwrap_or_default();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
    let value = r.cookie("session").unwrap_or_default();
    let session = app
        .api(
            "GET",
            "/api/session",
            Some(&format!("session={value}")),
            None,
            None,
        )
        .await;
    let body = json(&session);
    assert_eq!(body["user"]["name"], "alice (dev)");
    assert_eq!(body["user"]["id"], "1000000000000000001");
    assert_eq!(body["csrf"], app.csrf_of(&value));
    assert_eq!(body["guilds"][0]["name"], "Alice Dev Server");
}

#[tokio::test]
async fn is_admin_matches_the_configured_id_exactly_and_nobody_else() {
    let app = App::with_admin("1000000000000000001");
    let (alice, _) = app.dev_login("alice").await;
    let (bob, _) = app.dev_login("bob").await;
    let alice_session = json(
        &app.api("GET", "/api/session", Some(&alice), None, None)
            .await,
    );
    assert_eq!(alice_session["is_admin"], true);
    let bob_session = json(&app.api("GET", "/api/session", Some(&bob), None, None).await);
    assert_eq!(bob_session["is_admin"], false);
}

#[tokio::test]
async fn dev_login_is_refused_when_off_and_for_unknown_users() {
    let off = App::new(false, true);
    let r = off
        .api(
            "POST",
            "/api/dev-login",
            None,
            None,
            Some(r#"{"who":"alice"}"#),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let on = App::new(true, false);
    let r = on
        .api(
            "POST",
            "/api/dev-login",
            None,
            None,
            Some(r#"{"who":"mallory"}"#),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&r)["error"]["code"], "unknown_user");
    assert!(r.cookie("session").is_none());
}

#[tokio::test]
async fn private_endpoints_need_a_session() {
    let app = App::new(true, false);
    for (method, path) in [
        ("GET", "/api/bots"),
        ("POST", "/api/bots"),
        ("GET", "/api/bots/1"),
    ] {
        let r = app.api(method, path, None, None, Some("{}")).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(json(&r)["error"]["code"], "unauthorized");
    }
}

#[tokio::test]
async fn every_write_needs_the_form_token_header() {
    let app = App::new(true, false);
    let (cookie, csrf) = app.dev_login("alice").await;
    let none = app
        .api(
            "POST",
            "/api/bots",
            Some(&cookie),
            None,
            Some(r#"{"name":"Alice Bot"}"#),
        )
        .await;
    assert_eq!(none.status, StatusCode::FORBIDDEN);
    assert_eq!(json(&none)["error"]["code"], "csrf");
    let wrong = create(&app, &cookie, "wrong", "Alice Bot").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let ok = create(&app, &cookie, &csrf, "Alice Bot").await;
    assert_eq!(ok.status, StatusCode::CREATED);
    assert_eq!(json(&ok)["id"], 1);
    // Une lecture ne demande pas le jeton.
    let read = app.api("GET", "/api/bots", Some(&cookie), None, None).await;
    assert_eq!(read.status, StatusCode::OK);
}

#[tokio::test]
async fn bot_names_follow_the_rule_and_the_limit() {
    let app = App::with_limits(true, false, 2, 0);
    let (cookie, csrf) = app.dev_login("alice").await;
    for bad in ["ab", "x".repeat(33).as_str(), "<b>"] {
        let r = create(&app, &cookie, &csrf, bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(json(&r)["error"]["code"], "invalid_name");
    }
    assert_eq!(
        create(&app, &cookie, &csrf, "First Bot").await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        create(&app, &cookie, &csrf, "Second Bot").await.status,
        StatusCode::CREATED
    );
    let third = create(&app, &cookie, &csrf, "Third Bot").await;
    assert_eq!(third.status, StatusCode::FORBIDDEN);
    assert_eq!(json(&third)["error"]["code"], "limit_reached");
}

#[tokio::test]
async fn the_list_has_my_bots_only_with_their_two_versions() {
    let app = App::new(true, false);
    let (alice, a_csrf) = app.dev_login("alice").await;
    let (bob, b_csrf) = app.dev_login("bob").await;
    create(&app, &alice, &a_csrf, "Alice Bot").await;
    create(&app, &bob, &b_csrf, "Bob Bot").await;
    app.seed_bot(1, "free", "tok", "111");
    let r = app.api("GET", "/api/bots", Some(&alice), None, None).await;
    let body = json(&r);
    let bots = body["bots"].as_array().cloned().unwrap_or_default();
    assert_eq!(bots.len(), 1);
    assert_eq!(bots[0]["name"], "Alice Bot");
    assert_eq!(bots[0]["free"]["has_credential"], true);
    assert_eq!(bots[0]["free"]["app_name"], "Bot 111");
    assert_eq!(bots[0]["free"]["crash_looping"], false);
    assert_eq!(bots[0]["premium"]["has_credential"], false);
    assert_eq!(bots[0]["premium_subscribers"], 0);
    assert!(bots[0]["slot"].as_str().is_some());
    assert!(body["now"].as_i64().is_some());
    assert!(!r.body.contains("Bob Bot") && !r.body.contains("tok\""));
}

// Un container qui boucle en redémarrage signale souvent un jeton Discord invalide ou réinitialisé
// (approximation : vu depuis `docker inspect`, pas depuis un 401 de la Gateway).
#[tokio::test]
async fn a_crash_looping_container_is_flagged_on_the_dashboard_list() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "tok", "111");
    if let Ok(mut states) = app.containers.states.lock() {
        states.insert(
            "bot-1-free".to_string(),
            platform_runner::ContainerState::Restarting,
        );
    }
    let bots = json(&app.api("GET", "/api/bots", Some(&alice), None, None).await)["bots"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(bots[0]["free"]["crash_looping"], true);
    assert_eq!(bots[0]["premium"]["crash_looping"], false);
}

// Sans jeton (le container n'a jamais pu être lancé), on n'interroge même pas Docker : l'absence de
// jeton est déjà le bon diagnostic, pas la peine d'en chercher un deuxième.
#[tokio::test]
async fn without_a_credential_the_flag_is_never_set_even_if_docker_reports_restarting() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    if let Ok(mut states) = app.containers.states.lock() {
        states.insert(
            "bot-1-free".to_string(),
            platform_runner::ContainerState::Restarting,
        );
    }
    let bots = json(&app.api("GET", "/api/bots", Some(&alice), None, None).await)["bots"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(bots[0]["free"]["has_credential"], false);
    assert_eq!(bots[0]["free"]["crash_looping"], false);
}

#[tokio::test]
async fn the_overview_has_versions_containers_and_no_secret() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    app.seed_bot(1, "free", "free-secret-token", "111");
    let r = app
        .api("GET", "/api/bots/1", Some(&alice), None, None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let body = json(&r);
    assert_eq!(body["name"], "Alice Bot");
    let versions = body["versions"].as_array().cloned().unwrap_or_default();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["tier"], "free");
    assert_eq!(versions[0]["app"]["client_id"], "111");
    assert!(versions[0]["app"]["invite_url"]
        .as_str()
        .unwrap_or_default()
        .contains("client_id=111"));
    assert!(versions[1]["app"].is_null() && versions[1]["source"].is_null());
    assert_eq!(versions[0]["container"]["status"], "none");
    assert!(!r.body.contains("free-secret-token") && !r.body.contains("token_sealed"));
}

#[tokio::test]
async fn the_overview_reports_containers_and_escapes_nothing_it_should_not_trust() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    if let Ok(mut states) = app.containers.states.lock() {
        states.insert(
            "bot-1-free".to_string(),
            platform_runner::ContainerState::Running,
        );
        states.insert(
            "bot-1-premium".to_string(),
            platform_runner::ContainerState::Exited(3),
        );
    }
    if let Ok(mut logs) = app.containers.logs.lock() {
        *logs = "<script>alert(1)</script>".to_string();
    }
    let body = json(
        &app.api("GET", "/api/bots/1", Some(&alice), None, None)
            .await,
    );
    assert_eq!(body["versions"][0]["container"]["status"], "running");
    assert_eq!(body["versions"][1]["container"]["status"], "exited");
    assert_eq!(body["versions"][1]["container"]["exit_code"], 3);
    // Du texte brut dans du JSON : c'est au frontend de ne jamais l'insérer comme HTML.
    assert_eq!(body["versions"][0]["logs"], "<script>alert(1)</script>");
}

#[tokio::test]
async fn nobody_else_can_read_a_bot_through_the_api() {
    let app = App::new(true, false);
    let (alice, csrf) = app.dev_login("alice").await;
    let (bob, _) = app.dev_login("bob").await;
    create(&app, &alice, &csrf, "Alice Bot").await;
    for id in [1, 99] {
        let r = app
            .api("GET", &format!("/api/bots/{id}"), Some(&bob), None, None)
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{id}");
        assert_eq!(json(&r)["error"]["code"], "not_found");
    }
}

#[tokio::test]
async fn logging_out_needs_the_token_and_ends_the_session() {
    let app = App::new(true, false);
    let (cookie, csrf) = app.dev_login("alice").await;
    let refused = app
        .api("POST", "/api/logout", Some(&cookie), None, None)
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let ok = app
        .api("POST", "/api/logout", Some(&cookie), Some(&csrf), None)
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert!(ok.set_cookies().iter().any(|c| c.contains("Max-Age=0")));
    let after = app.api("GET", "/api/bots", Some(&cookie), None, None).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sessions_lists_only_mine_marks_the_current_one_and_lets_me_revoke_another() {
    let app = App::new(true, false);
    let (cookie_a, csrf_a) = app.dev_login("alice").await;
    let (cookie_b, _) = app.dev_login("alice").await;
    let (bob_cookie, bob_csrf) = app.dev_login("bob").await;
    let list = json(
        &app.api("GET", "/api/sessions", Some(&cookie_a), None, None)
            .await,
    );
    let sessions = list["sessions"].as_array().cloned().unwrap_or_default();
    // Les deux connexions d'alice, pas celle de bob.
    assert_eq!(sessions.len(), 2);
    let current: Vec<bool> = sessions.iter().map(|s| s["is_current"] == true).collect();
    assert_eq!(current.iter().filter(|c| **c).count(), 1, "{sessions:?}");
    let other_id = sessions
        .iter()
        .find(|s| s["is_current"] != true)
        .and_then(|s| s["id"].as_str())
        .unwrap_or_default()
        .to_string();
    // La session de bob n'apparaît jamais, et il ne peut pas révoquer celle d'alice.
    let bob_list = json(
        &app.api("GET", "/api/sessions", Some(&bob_cookie), None, None)
            .await,
    );
    assert_eq!(bob_list["sessions"].as_array().map(Vec::len), Some(1));
    let refused = app
        .api(
            "POST",
            &format!("/api/sessions/{other_id}/revoke"),
            Some(&bob_cookie),
            Some(&bob_csrf),
            None,
        )
        .await;
    assert_eq!(refused.status, StatusCode::NO_CONTENT); // ne révèle pas si l'id existe
    assert_eq!(
        app.api("GET", "/api/bots", Some(&cookie_b), None, None)
            .await
            .status,
        StatusCode::OK
    );
    // Alice révoque sa propre autre session : elle cesse de fonctionner, la première reste valide.
    let revoked = app
        .api(
            "POST",
            &format!("/api/sessions/{other_id}/revoke"),
            Some(&cookie_a),
            Some(&csrf_a),
            None,
        )
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.api("GET", "/api/bots", Some(&cookie_b), None, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.api("GET", "/api/bots", Some(&cookie_a), None, None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn api_answers_are_never_cached_and_carry_the_security_headers() {
    let app = App::new(true, false);
    let r = app.api("GET", "/api/session", None, None, None).await;
    assert_eq!(
        r.headers.get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        r.headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
}

mod support;

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use platform_core::ProductId;
use platform_github::Installation;
use platform_runner::ContainerState;
use platform_store::{DeployStatus, StoredInstallation};
use ring::hmac;
use support::*;

const ALICE: u64 = 1_000_000_000_000_000_001;
const REPO: &str = "alice/bot";
const FREE_TOKEN: &str = "free.bot.token";
const PREMIUM_TOKEN: &str = "premium.bot.token";

struct Scene {
    app: App,
    alice: (String, String),
}

fn installation(id: u64) -> StoredInstallation {
    StoredInstallation {
        id,
        login: "alice".to_string(),
        selection: "selected".to_string(),
    }
}

// Alice a un bot (1) avec ses deux jetons Discord, une installation GitHub (7) et un dépôt à deux branches.
async fn scene(cooldown: u32) -> Scene {
    let app = App::with_limits(true, false, 10, cooldown);
    let alice = app.dev_login("alice").await;
    app.post(
        "/products",
        Some(&alice.0),
        &format!("name=Alice+Bot&csrf={}", alice.1),
    )
    .await;
    app.seed_bot(1, "free", FREE_TOKEN, "111");
    app.seed_bot(1, "premium", PREMIUM_TOKEN, "222");
    app.github
        .offer(7, REPO, &[("main", HEAD_A), ("dev", HEAD_B)]);
    let stored = app
        .store
        .lock()
        .is_ok_and(|s| s.replace_installations(ALICE, &[installation(7)]).is_ok());
    assert!(stored);
    Scene { app, alice }
}

fn source_form(s: &Scene, extra: &str) -> String {
    format!(
        "csrf={}&installation=7&repo={}&branch=main{extra}",
        s.alice.1,
        form_encode(REPO)
    )
}

async fn connect(s: &Scene, tier: &str, extra: &str) -> Resp {
    s.app
        .post(
            &format!("/products/1/source/{tier}"),
            Some(&s.alice.0),
            &source_form(s, extra),
        )
        .await
}

async fn deploy(s: &Scene, tier: &str, branch: &str) -> Resp {
    s.app
        .post(
            "/products/1/deployments",
            Some(&s.alice.0),
            &format!("csrf={}&tier={tier}&branch={branch}", s.alice.1),
        )
        .await
}

fn deployments(app: &App) -> Vec<platform_store::Deployment> {
    app.store
        .lock()
        .ok()
        .and_then(|s| s.deployments_of(ProductId(1), 100).ok())
        .unwrap_or_default()
}

fn status_of(app: &App, id: u64) -> Option<DeployStatus> {
    app.store
        .lock()
        .ok()
        .and_then(|s| s.deployment(id).ok().flatten())
        .map(|d| d.status)
}

fn sign(body: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, WEBHOOK_SECRET.as_bytes());
    let tag = hmac::sign(&key, body.as_bytes());
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

async fn webhook(app: &App, event: &str, body: &str, signature: Option<&str>) -> StatusCode {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let mut request = Request::builder()
        .method("POST")
        .uri("/github/webhook")
        .header("x-github-event", event);
    if let Some(sig) = signature {
        request = request.header("x-hub-signature-256", sig);
    }
    let Ok(request) = request.body(Body::from(body.to_string())) else {
        panic!("bad request");
    };
    let Ok(response) = app.router.clone().oneshot(request).await;
    response.status()
}

fn push_body(branch: &str, sha: &str) -> String {
    format!(
        r#"{{"ref":"refs/heads/{branch}","after":"{sha}","deleted":false,"repository":{{"full_name":"{REPO}"}},"installation":{{"id":7}}}}"#
    )
}

// ---------- Connexion GitHub ----------

#[tokio::test]
async fn the_github_page_explains_when_github_is_not_set_up() {
    let s = scene(0).await;
    s.app.github.configured.store(false, Ordering::SeqCst);
    let page = s.app.get("/github", Some(&s.alice.0)).await;
    assert!(page.body.contains("not set up") && page.body.contains("SETUP_GITHUB"));
    let connect = s.app.get("/github/connect", Some(&s.alice.0)).await;
    assert_eq!(connect.status, StatusCode::SERVICE_UNAVAILABLE);
    let wizard = s.app.get("/products/1/source/free", Some(&s.alice.0)).await;
    assert_eq!(wizard.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn the_github_page_lists_accounts_and_offers_both_buttons() {
    let s = scene(0).await;
    let page = s.app.get("/github", Some(&s.alice.0)).await.body;
    assert!(page.contains("alice") && page.contains("selected repositories only"));
    assert!(page.contains(r#"href="/github/install?next=/github""#));
    assert!(page.contains(r#"href="/github/connect?next=/github""#));
    assert!(
        page.contains(r#"class="mono">alice/bot</span>"#)
            || page.contains("No repository is selected")
    );
    let (bob, _) = s.app.dev_login("bob").await;
    let empty = s.app.get("/github", Some(&bob)).await.body;
    assert!(empty.contains("No GitHub account connected yet"));
}

#[tokio::test]
async fn the_authorization_round_trip_stores_the_users_installations() {
    let s = scene(0).await;
    if let Ok(mut installs) = s.app.github.installs.lock() {
        *installs = Ok(vec![
            Installation {
                id: 9,
                login: "alice".to_string(),
                selection: "all".to_string(),
            },
            Installation {
                id: 10,
                login: "org".to_string(),
                selection: "selected".to_string(),
            },
        ]);
    }
    let start = s.app.get("/github/connect", Some(&s.alice.0)).await;
    assert_eq!(start.status, StatusCode::SEE_OTHER);
    let location = start.location().unwrap_or_default().to_string();
    assert!(
        location.starts_with("https://github.test/authorize?state="),
        "{location}"
    );
    let state = location.rsplit('=').next().unwrap_or_default().to_string();
    let cookie = start.cookie("github_state").unwrap_or_default();
    assert_eq!(cookie, state);
    let back = s
        .app
        .get(
            &format!("/github/callback?code=good&state={state}"),
            Some(&format!("{}; github_state={cookie}", s.alice.0)),
        )
        .await;
    assert_eq!(back.location(), Some("/github"));
    let stored = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.installations_of(ALICE).ok());
    assert_eq!(
        stored.map(|v| v.into_iter().map(|i| i.id).collect::<Vec<_>>()),
        Some(vec![9, 10])
    );
}

#[tokio::test]
async fn a_forged_or_missing_state_and_a_refused_code_change_nothing() {
    let s = scene(0).await;
    let cookie = format!("{}; github_state=real", s.alice.0);
    for (path, cookie) in [
        ("/github/callback?code=good&state=forged", cookie.as_str()),
        ("/github/callback?code=good&state=real", s.alice.0.as_str()),
        ("/github/callback?code=good", cookie.as_str()),
        ("/github/callback?error=access_denied", cookie.as_str()),
    ] {
        let r = s.app.get(path, Some(cookie)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}");
    }
    let refused = s
        .app
        .get("/github/callback?code=bad&state=real", Some(&cookie))
        .await;
    assert_eq!(refused.status, StatusCode::BAD_GATEWAY);
    let kept = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.installations_of(ALICE).ok());
    assert_eq!(kept.map(|v| v.len()), Some(1));
}

// ---------- Choix du dépôt ----------

#[tokio::test]
async fn the_wizard_skips_the_account_step_when_there_is_only_one() {
    let s = scene(0).await;
    let repos = s.app.get("/products/1/source/free", Some(&s.alice.0)).await;
    assert_eq!(repos.status, StatusCode::OK);
    assert!(repos.body.contains(&format!("repo={REPO}")) && repos.body.contains("Private"));
    let branches = s
        .app
        .get(
            &format!(
                "/products/1/source/free?installation=7&repo={}",
                form_encode(REPO)
            ),
            Some(&s.alice.0),
        )
        .await
        .body;
    assert!(
        branches.contains(r#"<option value="main" selected>main</option>"#),
        "{branches}"
    );
    assert!(branches.contains(r#"<option value="dev">dev</option>"#));
    assert!(branches.contains("both") && branches.contains("bot.toml"));
    let premium = s
        .app
        .get(
            &format!(
                "/products/1/source/premium?installation=7&repo={}",
                form_encode(REPO)
            ),
            Some(&s.alice.0),
        )
        .await
        .body;
    assert!(!premium.contains(r#"name="both""#));
}

#[tokio::test]
async fn the_wizard_asks_which_account_when_there_are_several_and_explains_when_there_is_none() {
    let s = scene(0).await;
    let two = s
        .app
        .store
        .lock()
        .map(|st| st.replace_installations(ALICE, &[installation(7), installation(8)]));
    assert!(matches!(two, Ok(Ok(()))));
    let page = s
        .app
        .get("/products/1/source/free", Some(&s.alice.0))
        .await
        .body;
    assert!(page.contains("installation=7") && page.contains("installation=8"));
    let none = s
        .app
        .store
        .lock()
        .map(|st| st.replace_installations(ALICE, &[]));
    assert!(matches!(none, Ok(Ok(()))));
    let page = s
        .app
        .get("/products/1/source/free", Some(&s.alice.0))
        .await
        .body;
    assert!(page.contains("No GitHub account is connected"));
    assert!(
        page.contains(r#"href="/github/install?next=/products/1/source/free""#),
        "{page}"
    );
    assert!(page.contains(r#"href="/github/connect?next=/products/1/source/free""#));
}

#[tokio::test]
async fn the_wizard_refuses_an_account_or_repository_the_user_does_not_have() {
    let s = scene(0).await;
    let other = s
        .app
        .get("/products/1/source/free?installation=99", Some(&s.alice.0))
        .await
        .body;
    assert!(
        other.contains("installation=7"),
        "an unknown account is ignored, the list is shown"
    );
    let missing = s
        .app
        .get(
            "/products/1/source/free?installation=7&repo=alice%2Fsecret",
            Some(&s.alice.0),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.body.contains("Repository not available"));
    let (bob, _) = s.app.dev_login("bob").await;
    assert_eq!(
        s.app
            .get("/products/1/source/free", Some(&bob))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .get("/products/1/source/gold", Some(&s.alice.0))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn github_failures_in_the_wizard_are_explained() {
    let s = scene(0).await;
    if let Ok(mut down) = s.app.github.down.lock() {
        *down = true;
    }
    let r = s.app.get("/products/1/source/free", Some(&s.alice.0)).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.body.contains("GitHub did not answer"));
}

#[tokio::test]
async fn saving_a_source_records_the_branch_head_so_auto_deploy_waits_for_the_next_push() {
    let s = scene(0).await;
    let r = connect(&s, "free", "&auto_deploy=1").await;
    assert_eq!(r.location(), Some("/products/1"));
    let saved = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten());
    let saved = saved.unwrap_or_else(|| panic!("no source saved"));
    assert_eq!(
        (
            saved.repo.as_str(),
            saved.branch.as_str(),
            saved.auto_deploy
        ),
        (REPO, "main", true)
    );
    assert_eq!(saved.last_head, HEAD_A);
    assert_eq!(saved.installation, 7);
    assert!(deployments(&s.app).is_empty());
    let manual = connect(&s, "free", "").await;
    assert_eq!(manual.location(), Some("/products/1"));
    let again = s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten());
    assert_eq!(again.map(|a| a.auto_deploy), Some(false));
}

#[tokio::test]
async fn one_repository_can_serve_both_versions() {
    let s = scene(0).await;
    connect(&s, "free", "&both=1").await;
    for tier in ["free", "premium"] {
        let src = s
            .app
            .store
            .lock()
            .ok()
            .and_then(|st| st.source(ProductId(1), tier).ok().flatten());
        assert_eq!(src.map(|x| x.repo), Some(REPO.to_string()), "{tier}");
    }
    let s2 = scene(0).await;
    connect(&s2, "premium", "&both=1").await;
    let free = s2
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten());
    assert!(free.is_none(), "the box only exists for the free version");
}

#[tokio::test]
async fn a_forged_source_form_is_refused_after_checking_with_github() {
    let s = scene(0).await;
    for form in [
        format!(
            "csrf={}&installation=99&repo={}&branch=main",
            s.alice.1,
            form_encode(REPO)
        ),
        format!(
            "csrf={}&installation=7&repo=alice%2Fsecret&branch=main",
            s.alice.1
        ),
        format!(
            "csrf={}&installation=7&repo={}&branch=nope",
            s.alice.1,
            form_encode(REPO)
        ),
        format!("csrf={}&installation=7&repo=..%2Fx&branch=main", s.alice.1),
        format!(
            "csrf={}&installation=7&repo={}&branch=a..b",
            s.alice.1,
            form_encode(REPO)
        ),
    ] {
        let r = s
            .app
            .post("/products/1/source/free", Some(&s.alice.0), &form)
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{form}");
    }
    let no_csrf = s
        .app
        .post(
            "/products/1/source/free",
            Some(&s.alice.0),
            &source_form(&s, "").replace(&s.alice.1, "wrong"),
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    let (bob, b_csrf) = s.app.dev_login("bob").await;
    let theirs = s
        .app
        .post(
            "/products/1/source/free",
            Some(&bob),
            &source_form(&s, "").replace(&s.alice.1, &b_csrf),
        )
        .await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND);
    assert!(s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten())
        .is_none());
}

#[tokio::test]
async fn disconnecting_removes_the_source_of_that_version_only() {
    let s = scene(0).await;
    connect(&s, "free", "&both=1").await;
    let r = s
        .app
        .post(
            "/products/1/source/free/remove",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1"));
    let get = |tier: &str| {
        s.app
            .store
            .lock()
            .ok()
            .and_then(|st| st.source(ProductId(1), tier).ok().flatten())
    };
    assert!(get("free").is_none() && get("premium").is_some());
    let bad = s
        .app
        .post(
            "/products/1/source/free/remove",
            Some(&s.alice.0),
            "csrf=wrong",
        )
        .await;
    assert_eq!(bad.status, StatusCode::FORBIDDEN);
}

// ---------- Déploiement ----------

#[tokio::test]
async fn deploying_a_branch_builds_its_head_and_starts_the_container_with_the_creators_token() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    let r = deploy(&s, "free", "main").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), Some("/products/1/deployments/1"));
    let requests = s.app.builder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        (
            requests[0].installation,
            requests[0].repo.as_str(),
            requests[0].sha.as_str(),
            requests[0].product,
            requests[0].tier.as_str()
        ),
        (7, REPO, HEAD_A, 1, "free")
    );
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Running));
    let specs = s
        .app
        .containers
        .deployed
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "bot-1-free");
    assert_eq!(
        specs[0].image,
        format!("platform-bot-1-free:{}", &HEAD_A[..12])
    );
    let env = |n: &str| {
        specs[0]
            .env
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(env("DISCORD_TOKEN").as_deref(), Some(FREE_TOKEN));
    let api = env("PLATFORM_API_TOKEN").unwrap_or_default();
    assert_eq!(s.app.product_for_token(&api).map(|p| p.0), Some(1));
    assert_eq!(env("TIER").as_deref(), Some("free"));
}

#[tokio::test]
async fn a_premium_deployment_uses_the_premium_token_and_container() {
    let s = scene(0).await;
    connect(&s, "premium", "").await;
    deploy(&s, "premium", "dev").await;
    let specs = s
        .app
        .containers
        .deployed
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    assert_eq!(specs[0].name, "bot-1-premium");
    assert!(specs[0]
        .env
        .iter()
        .any(|(k, v)| k == "DISCORD_TOKEN" && v == PREMIUM_TOKEN));
    assert_eq!(s.app.builder.requests()[0].sha, HEAD_B);
    assert_eq!(deployments(&s.app)[0].branch, "dev");
}

#[tokio::test]
async fn the_deployed_commit_is_the_one_github_reports_now_not_one_from_the_form() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    s.app.github.push(REPO, "main", HEAD_B);
    deploy(&s, "free", "main").await;
    assert_eq!(deployments(&s.app)[0].commit, HEAD_B);
    let unknown = deploy(&s, "free", "ghost").await;
    assert_eq!(unknown.status, StatusCode::BAD_GATEWAY);
    assert_eq!(deployments(&s.app).len(), 1);
}

#[tokio::test]
async fn a_new_deployment_replaces_the_live_one_which_stays_available() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    deploy(&s, "free", "main").await;
    s.app.github.push(REPO, "main", HEAD_B);
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Ready));
    assert_eq!(status_of(&s.app, 2), Some(DeployStatus::Running));
    let page = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(page.contains("state live") && page.contains("state ready"));
    assert!(page.contains(r#"state live"#) && page.contains(&HEAD_B[..7]));
}

#[tokio::test]
async fn a_failed_build_is_recorded_and_the_live_version_keeps_running() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    deploy(&s, "free", "main").await;
    let containers_before = s
        .app
        .containers
        .deployed
        .lock()
        .map(|d| d.len())
        .unwrap_or(0);
    if let Ok(mut failure) = s.app.builder.failure.lock() {
        *failure = Some("npm ERR! boom".to_string());
    }
    s.app.github.push(REPO, "main", HEAD_B);
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 2), Some(DeployStatus::Failed));
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Running));
    assert_eq!(
        s.app
            .containers
            .deployed
            .lock()
            .map(|d| d.len())
            .unwrap_or(0),
        containers_before
    );
    let detail = s
        .app
        .get("/products/1/deployments/2", Some(&s.alice.0))
        .await
        .body;
    assert!(detail.contains("Build failed") && detail.contains("npm ERR! boom"));
    assert!(detail.contains("state failed") && detail.contains("Redeploy"));
}

#[tokio::test]
async fn a_docker_failure_after_the_build_fails_the_deployment() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    if let Ok(mut fail) = s.app.containers.fail_deploy.lock() {
        *fail = true;
    }
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Failed));
    let log = deployments(&s.app)[0].log.clone();
    assert!(log.contains("Docker did not start the container"), "{log}");
}

// Vérification d'acceptation : un `docker run` réussi ne veut pas dire que le
// bot tourne. Si le container a déjà quitté quand la plateforme vérifie, le déploiement échoue
// avec les dernières lignes de journal au lieu d'afficher « Live » pour un bot déjà mort.
#[tokio::test]
async fn a_container_that_exits_immediately_fails_the_deployment() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    if let Ok(mut states) = s.app.containers.states.lock() {
        states.insert("bot-1-free".to_string(), ContainerState::Exited(1));
    }
    if let Ok(mut logs) = s.app.containers.logs.lock() {
        *logs = "Error: TOKEN_INVALID".to_string();
    }
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Failed));
    let log = deployments(&s.app)[0].log.clone();
    assert!(log.contains("exited immediately (code 1)"), "{log}");
    assert!(log.contains("TOKEN_INVALID"), "{log}");
}

#[tokio::test]
async fn a_container_stuck_restarting_fails_the_deployment() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    if let Ok(mut states) = s.app.containers.states.lock() {
        states.insert("bot-1-free".to_string(), ContainerState::Restarting);
    }
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Failed));
    let log = deployments(&s.app)[0].log.clone();
    assert!(log.contains("stuck restarting"), "{log}");
}

#[tokio::test]
async fn a_container_that_is_simply_running_is_accepted_as_before() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    if let Ok(mut states) = s.app.containers.states.lock() {
        states.insert("bot-1-free".to_string(), ContainerState::Running);
    }
    deploy(&s, "free", "main").await;
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Running));
}

#[tokio::test]
async fn a_version_without_its_discord_bot_cannot_be_built() {
    let s = scene(0).await;
    let app = App::with_limits(true, false, 10, 0);
    let (alice, csrf) = app.dev_login("alice").await;
    app.post(
        "/products",
        Some(&alice),
        &format!("name=Alice+Bot&csrf={csrf}"),
    )
    .await;
    app.github.offer(7, REPO, &[("main", HEAD_A)]);
    let stored = app
        .store
        .lock()
        .map(|st| st.replace_installations(ALICE, &[installation(7)]));
    assert!(matches!(stored, Ok(Ok(()))));
    app.post(
        "/products/1/source/free",
        Some(&alice),
        &format!(
            "csrf={csrf}&installation=7&repo={}&branch=main",
            form_encode(REPO)
        ),
    )
    .await;
    let r = app
        .post(
            "/products/1/deployments",
            Some(&alice),
            &format!("csrf={csrf}&tier=free&branch=main"),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(status_of(&app, 1), Some(DeployStatus::Failed));
    assert!(deployments(&app)[0]
        .log
        .contains("Connect this version's Discord bot"));
    assert!(app.builder.requests().is_empty());
    drop(s);
}

#[tokio::test]
async fn a_commit_that_was_already_built_is_not_built_again() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    if let Ok(mut images) = s.app.containers.images.lock() {
        images.insert(format!("platform-bot-1-free:{}", &HEAD_A[..12]));
    }
    deploy(&s, "free", "main").await;
    assert!(s.app.builder.requests().is_empty());
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Running));
    assert!(deployments(&s.app)[0].log.contains("reusing its image"));
}

#[tokio::test]
async fn going_back_redeploys_the_old_commit_without_touching_the_branch_tracking() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    deploy(&s, "free", "main").await;
    s.app.github.push(REPO, "main", HEAD_B);
    deploy(&s, "free", "main").await;
    let r = s
        .app
        .post(
            "/products/1/deployments/1/redeploy",
            Some(&s.alice.0),
            &format!("csrf={}", s.alice.1),
        )
        .await;
    assert_eq!(r.location(), Some("/products/1/deployments/3"));
    let all = deployments(&s.app);
    assert_eq!(
        (all[0].commit.as_str(), all[0].trigger.as_str()),
        (HEAD_A, "redeploy")
    );
    assert_eq!(status_of(&s.app, 3), Some(DeployStatus::Running));
    assert_eq!(status_of(&s.app, 2), Some(DeployStatus::Ready));
    // Le sondage ne « corrige » pas le retour en arrière : la tête de la branche est déjà connue.
    platform_web::poll_once(&s.app.state);
    assert_eq!(deployments(&s.app).len(), 3);
}

#[tokio::test]
async fn deploying_is_rate_limited_per_container() {
    let s = scene(30).await;
    connect(&s, "free", "").await;
    assert_eq!(
        deploy(&s, "free", "main").await.status,
        StatusCode::SEE_OTHER
    );
    let again = deploy(&s, "free", "main").await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(again.body.contains("Wait 30 more second(s)"));
    assert_eq!(deployments(&s.app).len(), 1);
    s.app.advance(30);
    assert_eq!(
        deploy(&s, "free", "main").await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn deploying_needs_a_repository_the_form_token_and_the_owner() {
    let s = scene(0).await;
    let none = deploy(&s, "free", "main").await;
    assert_eq!(none.status, StatusCode::CONFLICT);
    connect(&s, "free", "").await;
    let bad_csrf = s
        .app
        .post(
            "/products/1/deployments",
            Some(&s.alice.0),
            "csrf=x&tier=free&branch=main",
        )
        .await;
    assert_eq!(bad_csrf.status, StatusCode::FORBIDDEN);
    let (bob, b_csrf) = s.app.dev_login("bob").await;
    let theirs = s
        .app
        .post(
            "/products/1/deployments",
            Some(&bob),
            &format!("csrf={b_csrf}&tier=free&branch=main"),
        )
        .await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND);
    let tier = deploy(&s, "gold", "main").await;
    assert_eq!(tier.status, StatusCode::NOT_FOUND);
    assert!(deployments(&s.app).is_empty());
}

#[tokio::test]
async fn the_branch_chooser_lists_branches_with_their_commit() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    let page = s
        .app
        .get("/products/1/deploy/free", Some(&s.alice.0))
        .await
        .body;
    assert!(
        page.contains(r#"<option value="main" selected>main (aaaaaaa)</option>"#),
        "{page}"
    );
    assert!(page.contains(r#"<option value="dev">dev (bbbbbbb)</option>"#));
    let (bob, _) = s.app.dev_login("bob").await;
    assert_eq!(
        s.app
            .get("/products/1/deploy/free", Some(&bob))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .get("/products/1/deploy/premium", Some(&s.alice.0))
            .await
            .status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn deployment_pages_belong_to_their_bot_and_their_owner() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    deploy(&s, "free", "main").await;
    let ok = s
        .app
        .get("/products/1/deployments/1", Some(&s.alice.0))
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(
        ok.body.contains("state live") && ok.body.contains("aaaaaaa") && ok.body.contains("manual")
    );
    assert!(ok.body.contains("Commit aaaaaaa on main"));
    let (bob, bob_csrf) = s.app.dev_login("bob").await;
    assert_eq!(
        s.app
            .get("/products/1/deployments/1", Some(&bob))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.app
            .get("/products/1/deployments/99", Some(&s.alice.0))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let redo = s
        .app
        .post(
            "/products/1/deployments/1/redeploy",
            Some(&bob),
            &format!("csrf={bob_csrf}"),
        )
        .await;
    assert_eq!(redo.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_refresh_faster_while_something_is_building() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    let idle = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(idle.contains(r#"data-poll="6""#) && idle.contains("Not deployed"));
    let queued = s
        .app
        .store
        .lock()
        .map(|st| st.create_deployment(ProductId(1), "free", HEAD_A, "main", "manual", 1));
    assert!(matches!(queued, Ok(Ok(_))));
    let busy = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(busy.contains(r#"data-poll="2""#) && busy.contains("Queued"));
    let detail = s
        .app
        .get("/products/1/deployments/1", Some(&s.alice.0))
        .await
        .body;
    assert!(detail.contains(r#"data-poll="2""#) && detail.contains("Waiting for a build slot"));
}

#[tokio::test]
async fn the_project_page_shows_the_repository_the_bot_and_the_right_actions() {
    let s = scene(0).await;
    let bare = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(bare.contains("Connect a repository") && bare.contains("Run the example bot"));
    assert!(!bare.contains("Deploy latest"));
    connect(&s, "free", "&auto_deploy=1").await;
    let linked = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    assert!(linked.contains("alice/bot") && linked.contains("On for pushes to"));
    assert!(linked.contains("Deploy latest") && linked.contains("Choose a branch"));
    assert!(linked.contains("Bot 111"));
    let source = s.app.get("/products/1/source", Some(&s.alice.0)).await.body;
    assert!(source.contains("Disconnect") && source.contains("alice/bot"));
    assert!(linked.contains(r#"data-live="prod""#) && linked.contains(r#"data-live="history""#));
    assert!(!linked.contains(FREE_TOKEN));
}

#[tokio::test]
async fn escaping_covers_branch_and_repository_text_on_every_new_page() {
    let s = scene(0).await;
    let evil = "<img src=x>";
    let stored = s.app.store.lock().map(|st| {
        st.save_source(
            ProductId(1),
            "free",
            &platform_store::Source {
                installation: 7,
                repo: evil.to_string(),
                branch: evil.to_string(),
                auto_deploy: false,
                last_head: String::new(),
            },
        )
    });
    assert!(matches!(stored, Ok(Ok(()))));
    let queued = s
        .app
        .store
        .lock()
        .map(|st| st.create_deployment(ProductId(1), "free", evil, evil, evil, 1));
    assert!(matches!(queued, Ok(Ok(_))));
    for path in ["/products/1", "/products/1/deployments/1"] {
        let body = s.app.get(path, Some(&s.alice.0)).await.body;
        assert!(!body.contains("<img src=x>"), "{path}");
        assert!(body.contains("&lt;img src=x&gt;"), "{path}");
    }
}

// ---------- Webhook et sondage ----------

#[tokio::test]
async fn webhooks_need_a_valid_signature() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    let body = push_body("main", HEAD_B);
    assert_eq!(
        webhook(&s.app, "push", &body, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        webhook(&s.app, "push", &body, Some("sha256=00")).await,
        StatusCode::UNAUTHORIZED
    );
    let other = sign("something else");
    assert_eq!(
        webhook(&s.app, "push", &body, Some(&other)).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(deployments(&s.app).is_empty());
}

#[tokio::test]
async fn a_signed_ping_is_answered_and_unknown_events_are_ignored() {
    let s = scene(0).await;
    let body = "{}";
    assert_eq!(
        webhook(&s.app, "ping", body, Some(&sign(body))).await,
        StatusCode::OK
    );
    assert_eq!(
        webhook(&s.app, "star", body, Some(&sign(body))).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_push_to_a_watched_branch_deploys_once_even_if_the_delivery_is_replayed() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    let body = push_body("main", HEAD_B);
    assert_eq!(
        webhook(&s.app, "push", &body, Some(&sign(&body))).await,
        StatusCode::ACCEPTED
    );
    let all = deployments(&s.app);
    assert_eq!(all.len(), 1);
    assert_eq!(
        (
            all[0].commit.as_str(),
            all[0].trigger.as_str(),
            all[0].status
        ),
        (HEAD_B, "push", DeployStatus::Running)
    );
    assert_eq!(
        webhook(&s.app, "push", &body, Some(&sign(&body))).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(deployments(&s.app).len(), 1);
}

#[tokio::test]
async fn pushes_that_should_not_deploy_do_nothing() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    let other_branch = push_body("dev", HEAD_B);
    let tag = r#"{"ref":"refs/tags/v1","after":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repository":{"full_name":"alice/bot"},"installation":{"id":7}}"#;
    let other_repo = push_body("main", HEAD_B).replace(REPO, "alice/other");
    let other_install = push_body("main", HEAD_B).replace(r#""id":7"#, r#""id":8"#);
    for body in [other_branch, tag.to_string(), other_repo, other_install] {
        assert!(webhook(&s.app, "push", &body, Some(&sign(&body)))
            .await
            .is_success());
    }
    assert!(deployments(&s.app).is_empty());
    connect(&s, "free", "").await;
    let body = push_body("main", HEAD_B);
    webhook(&s.app, "push", &body, Some(&sign(&body))).await;
    assert!(
        deployments(&s.app).is_empty(),
        "manual-only sources are never auto-deployed"
    );
}

#[tokio::test]
async fn one_push_updates_every_version_that_follows_that_branch() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1&both=1").await;
    let body = push_body("main", HEAD_B);
    webhook(&s.app, "push", &body, Some(&sign(&body))).await;
    let mut tiers: Vec<_> = deployments(&s.app).into_iter().map(|d| d.tier).collect();
    tiers.sort();
    assert_eq!(tiers, ["free", "premium"]);
}

#[tokio::test]
async fn polling_deploys_new_commits_once_and_ignores_unchanged_branches() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    platform_web::poll_once(&s.app.state);
    assert!(
        deployments(&s.app).is_empty(),
        "enabling auto-deploy does not deploy by itself"
    );
    s.app.github.push(REPO, "main", HEAD_B);
    platform_web::poll_once(&s.app.state);
    assert_eq!(deployments(&s.app).len(), 1);
    assert_eq!(deployments(&s.app)[0].commit, HEAD_B);
    platform_web::poll_once(&s.app.state);
    assert_eq!(deployments(&s.app).len(), 1);
}

#[tokio::test]
async fn polling_does_not_retry_a_commit_that_failed_to_build() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    if let Ok(mut failure) = s.app.builder.failure.lock() {
        *failure = Some("boom".to_string());
    }
    s.app.github.push(REPO, "main", HEAD_B);
    platform_web::poll_once(&s.app.state);
    platform_web::poll_once(&s.app.state);
    platform_web::poll_once(&s.app.state);
    assert_eq!(deployments(&s.app).len(), 1);
    assert_eq!(status_of(&s.app, 1), Some(DeployStatus::Failed));
}

#[tokio::test]
async fn polling_survives_github_being_down_and_skips_manual_sources() {
    let s = scene(0).await;
    connect(&s, "free", "&auto_deploy=1").await;
    connect(&s, "premium", "").await;
    s.app.github.push(REPO, "main", HEAD_B);
    if let Ok(mut down) = s.app.github.down.lock() {
        *down = true;
    }
    platform_web::poll_once(&s.app.state);
    assert!(deployments(&s.app).is_empty());
    if let Ok(mut down) = s.app.github.down.lock() {
        *down = false;
    }
    platform_web::poll_once(&s.app.state);
    let tiers: Vec<_> = deployments(&s.app).into_iter().map(|d| d.tier).collect();
    assert_eq!(tiers, ["free"]);
}

#[tokio::test]
async fn deployments_interrupted_by_a_restart_are_marked_failed() {
    let s = scene(0).await;
    let ids: Vec<u64> = {
        let store = s.app.store.lock();
        let Ok(store) = store else { panic!("lock") };
        (0..3)
            .map(|i| {
                store
                    .create_deployment(ProductId(1), "free", HEAD_A, "main", "manual", i)
                    .unwrap_or(0)
            })
            .collect()
    };
    let moved = s
        .app
        .store
        .lock()
        .map(|st| st.set_deployment_progress(ids[1], DeployStatus::Building, "", "step", None));
    assert!(matches!(moved, Ok(Ok(()))));
    let done = s
        .app
        .store
        .lock()
        .map(|st| st.set_deployment_progress(ids[2], DeployStatus::Ready, "", "", Some(5)));
    assert!(matches!(done, Ok(Ok(()))));
    platform_web::recover_interrupted(&s.app.state);
    assert_eq!(status_of(&s.app, ids[0]), Some(DeployStatus::Failed));
    assert_eq!(status_of(&s.app, ids[1]), Some(DeployStatus::Failed));
    assert_eq!(status_of(&s.app, ids[2]), Some(DeployStatus::Ready));
    let log = deployments(&s.app)
        .into_iter()
        .find(|d| d.id == ids[1])
        .map(|d| d.log)
        .unwrap_or_default();
    assert!(log.contains("step") && log.contains("Interrupted"));
}

#[tokio::test]
async fn assets_are_served_with_the_right_types_and_the_policy_allows_only_our_script() {
    let s = scene(0).await;
    let js = s.app.get("/static/app.js", None).await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .starts_with("text/javascript"));
    assert!(js.body.contains("data-live") && js.body.contains("DOMParser"));
    let csp = js
        .headers
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(csp.contains("script-src 'self'") && csp.contains("connect-src 'self'"));
    assert!(!csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"));
    let page = s.app.get("/login", None).await.body;
    assert!(page.contains(r#"<script src="/static/app.js" defer></script>"#));
    assert!(!page.contains("onclick") && !page.contains("<script>"));
}

// ---------- Cas limites supplémentaires (mutants) ----------

#[tokio::test]
async fn the_same_callback_address_is_sent_to_github_twice() {
    let s = scene(0).await;
    let start = s.app.get("/github/connect", Some(&s.alice.0)).await;
    let state = start.cookie("github_state").unwrap_or_default();
    s.app
        .get(
            &format!("/github/callback?code=good&state={state}"),
            Some(&format!("{}; github_state={state}", s.alice.0)),
        )
        .await;
    let seen = s
        .app
        .github
        .redirects
        .lock()
        .map(|r| r.clone())
        .unwrap_or_default();
    assert_eq!(
        seen,
        [
            "http://127.0.0.1:8080/github/callback",
            "http://127.0.0.1:8080/github/callback"
        ]
    );
}

#[tokio::test]
async fn each_github_failure_in_the_wizard_has_its_own_message() {
    use platform_github::GitHubError;
    let s = scene(0).await;
    for (error, status, text) in [
        (
            GitHubError::NotFound,
            StatusCode::BAD_GATEWAY,
            "does not let the platform read this",
        ),
        (
            GitHubError::Unauthorized,
            StatusCode::BAD_GATEWAY,
            "does not let the platform read this",
        ),
        (
            GitHubError::RateLimited,
            StatusCode::TOO_MANY_REQUESTS,
            "limiting requests",
        ),
        (
            GitHubError::Parse,
            StatusCode::BAD_GATEWAY,
            "did not answer",
        ),
    ] {
        if let Ok(mut fail) = s.app.github.fail.lock() {
            *fail = Some(error.clone());
        }
        let r = s.app.get("/products/1/source/free", Some(&s.alice.0)).await;
        assert_eq!(r.status, status, "{error:?}");
        assert!(r.body.contains(text), "{error:?}: {}", r.body);
    }
}

#[tokio::test]
async fn a_repository_or_branch_is_refused_even_if_github_lists_it_when_the_name_is_unsafe() {
    let s = scene(0).await;
    s.app.github.offer(99, REPO, &[("main", HEAD_A)]);
    s.app.github.offer(7, "../x", &[("main", HEAD_A)]);
    s.app
        .github
        .offer(7, "alice/odd", &[("main", HEAD_A), ("a..b", HEAD_B)]);
    for form in [
        format!(
            "csrf={}&installation=99&repo={}&branch=main",
            s.alice.1,
            form_encode(REPO)
        ),
        format!("csrf={}&installation=7&repo=..%2Fx&branch=main", s.alice.1),
        format!(
            "csrf={}&installation=7&repo=alice%2Fodd&branch=a..b",
            s.alice.1
        ),
    ] {
        let r = s
            .app
            .post("/products/1/source/free", Some(&s.alice.0), &form)
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{form}");
    }
    assert!(s
        .app
        .store
        .lock()
        .ok()
        .and_then(|st| st.source(ProductId(1), "free").ok().flatten())
        .is_none());
}

fn headline(body: &str) -> String {
    let start = body.find(r#"class="prod-title""#).unwrap_or(0);
    let end = body[start..]
        .find("</dl>")
        .map_or(body.len(), |e| start + e);
    body[start..end].to_string()
}

#[tokio::test]
async fn the_headline_shows_the_live_deployment_even_when_a_newer_one_failed() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    deploy(&s, "free", "main").await;
    if let Ok(mut failure) = s.app.builder.failure.lock() {
        *failure = Some("boom".to_string());
    }
    s.app.github.push(REPO, "main", HEAD_B);
    deploy(&s, "free", "main").await;
    let page = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    let line = headline(&page);
    assert!(
        line.contains("Live") && line.contains("aaaaaaa") && !line.contains("bbbbbbb"),
        "{line}"
    );
}

#[tokio::test]
async fn each_version_only_shows_its_own_deployments_and_the_history_lists_both() {
    let s = scene(0).await;
    connect(&s, "free", "").await;
    deploy(&s, "free", "main").await;
    let page = s.app.get("/products/1", Some(&s.alice.0)).await.body;
    let (free, premium) = page.split_once(r#"id="premium""#).unwrap_or((&page, ""));
    assert!(headline(free).contains("aaaaaaa"));
    assert!(
        headline(premium).contains("Not deployed"),
        "{}",
        headline(premium)
    );
    let premium_rail = premium
        .split(r#"data-live="history""#)
        .next()
        .unwrap_or_default();
    assert!(!premium_rail.contains("cell live"));
    let history = page
        .split(r#"data-live="history""#)
        .nth(1)
        .unwrap_or_default();
    assert!(history.contains(r#"href="/products/1/deployments/1""#));
    assert!(
        history.contains("free version") && history.contains("manual"),
        "{history}"
    );
    assert!(history.contains("state live"));
}

// ---------- Retour automatique après l'installation de l'application ----------

fn return_cookie(r: &Resp) -> Option<String> {
    r.cookie("github_return")
}

#[tokio::test]
async fn the_install_link_remembers_where_to_come_back_to() {
    let s = scene(0).await;
    let r = s
        .app
        .get(
            "/github/install?next=/products/1/source/free",
            Some(&s.alice.0),
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        r.location(),
        Some("https://github.test/apps/test/installations/new")
    );
    assert_eq!(
        return_cookie(&r).as_deref(),
        Some("/products/1/source/free")
    );
    let plain = s.app.get("/github/install", Some(&s.alice.0)).await;
    assert_eq!(
        plain.location(),
        Some("https://github.test/apps/test/installations/new")
    );
    assert_eq!(return_cookie(&plain), None);
    let anonymous = s.app.get("/github/install?next=/github", None).await;
    assert_eq!(anonymous.location(), Some("/login"));
}

#[tokio::test]
async fn only_our_own_simple_paths_are_remembered() {
    let s = scene(0).await;
    for next in [
        "//evil.example/x",
        "https://evil.example",
        "/products/../etc",
        "/other",
        "/products/1?x=1",
        "/products/1;a",
        "products/1",
        "/products/1%2f",
        "",
    ] {
        let r = s
            .app
            .get(
                &format!("/github/install?next={}", form_encode(next)),
                Some(&s.alice.0),
            )
            .await;
        assert_eq!(return_cookie(&r), None, "{next:?}");
    }
    for ok in [
        "/github",
        "/dashboard",
        "/products/1",
        "/products/12/source/premium",
    ] {
        let r = s
            .app
            .get(&format!("/github/install?next={ok}"), Some(&s.alice.0))
            .await;
        assert_eq!(return_cookie(&r).as_deref(), Some(ok), "{ok}");
    }
}

#[tokio::test]
async fn the_setup_url_continues_with_the_authorization_without_a_new_screen() {
    let s = scene(0).await;
    let r = s
        .app
        .get(
            "/github/installed?installation_id=7&setup_action=install",
            Some(&s.alice.0),
        )
        .await;
    assert_eq!(r.location(), Some("/github/connect"));
    let anonymous = s.app.get("/github/installed", None).await;
    assert_eq!(anonymous.location(), Some("/login"));
}

// Une fois ramené par GitHub, le navigateur est sur l'adresse du serveur, jamais celle du
// frontend (DEC- aucun, bug corrigé 2026-11-11) : un chemin propre à l'app React doit devenir
// une adresse absolue vers `frontend_url`, sinon la redirection finale reste sur la mauvaise origine.
#[tokio::test]
async fn an_spa_only_return_path_becomes_absolute_once_frontend_url_is_set() {
    let s = App::with_frontend_url("http://127.0.0.1:5174");
    let (alice, _) = s.dev_login("alice").await;
    for (next, want) in [
        (
            "/projects/2/source/free",
            "http://127.0.0.1:5174/projects/2/source/free",
        ),
        ("/integrations", "http://127.0.0.1:5174/integrations"),
        // Une page servie par ce serveur lui-même reste relative : pas besoin de frontend_url.
        ("/github", "/github"),
    ] {
        let start = s
            .get(&format!("/github/connect?next={next}"), Some(&alice))
            .await;
        let state = start
            .location()
            .unwrap_or_default()
            .rsplit('=')
            .next()
            .unwrap_or_default()
            .to_string();
        let state_cookie = start.cookie("github_state").unwrap_or_default();
        let return_cookie = start.cookie("github_return").unwrap_or_default();
        let back = s
            .get(
                &format!("/github/callback?code=good&state={state}"),
                Some(&format!(
                    "{alice}; github_state={state_cookie}; github_return={return_cookie}"
                )),
            )
            .await;
        assert_eq!(back.location(), Some(want), "{next}");
    }
}

#[tokio::test]
async fn without_frontend_url_an_spa_only_return_path_stays_relative() {
    let s = App::new(true, false);
    let (alice, _) = s.dev_login("alice").await;
    let start = s
        .get("/github/connect?next=/projects/2/source/free", Some(&alice))
        .await;
    let state = start
        .location()
        .unwrap_or_default()
        .rsplit('=')
        .next()
        .unwrap_or_default()
        .to_string();
    let state_cookie = start.cookie("github_state").unwrap_or_default();
    let return_cookie = start.cookie("github_return").unwrap_or_default();
    let back = s
        .get(
            &format!("/github/callback?code=good&state={state}"),
            Some(&format!(
                "{alice}; github_state={state_cookie}; github_return={return_cookie}"
            )),
        )
        .await;
    // Documente le comportement sans `frontend_url` : reste relatif (pas de régression), mais
    // c'est exactement le cas qui casse en local si la variable n'est pas configurée.
    assert_eq!(back.location(), Some("/projects/2/source/free"));
}

#[tokio::test]
async fn connecting_can_also_remember_the_return_path() {
    let s = scene(0).await;
    let r = s
        .app
        .get(
            "/github/connect?next=/products/1/source/free",
            Some(&s.alice.0),
        )
        .await;
    assert_eq!(
        return_cookie(&r).as_deref(),
        Some("/products/1/source/free")
    );
    assert!(r.cookie("github_state").is_some());
}

async fn finish_authorization(s: &Scene, return_cookie_value: Option<&str>) -> Resp {
    let start = s.app.get("/github/connect", Some(&s.alice.0)).await;
    let state = start.cookie("github_state").unwrap_or_default();
    let mut cookie = format!("{}; github_state={state}", s.alice.0);
    if let Some(value) = return_cookie_value {
        cookie.push_str(&format!("; github_return={value}"));
    }
    s.app
        .get(
            &format!("/github/callback?code=good&state={state}"),
            Some(&cookie),
        )
        .await
}

#[tokio::test]
async fn after_authorizing_the_user_lands_where_they_started_and_the_cookies_are_cleared() {
    let s = scene(0).await;
    let r = finish_authorization(&s, Some("/products/1/source/free")).await;
    assert_eq!(r.location(), Some("/products/1/source/free"));
    let cleared = r.set_cookies();
    assert!(
        cleared
            .iter()
            .any(|c| c.starts_with("github_return=;") && c.contains("Max-Age=0")),
        "{cleared:?}"
    );
    assert!(cleared
        .iter()
        .any(|c| c.starts_with("github_state=;") && c.contains("Max-Age=0")));
}

#[tokio::test]
async fn a_tampered_return_cookie_falls_back_to_the_github_page() {
    let s = scene(0).await;
    for bad in [
        "//evil.example",
        "https://evil.example",
        "/other",
        "/products/../x",
    ] {
        let r = finish_authorization(&s, Some(bad)).await;
        assert_eq!(r.location(), Some("/github"), "{bad}");
    }
    assert_eq!(
        finish_authorization(&s, None).await.location(),
        Some("/github")
    );
}

#[tokio::test]
async fn the_github_page_shows_what_the_platform_can_really_read() {
    let s = scene(0).await;
    let seen = s.app.get("/github", Some(&s.alice.0)).await.body;
    assert!(seen.contains(r#"class="mono">alice/bot</span>"#), "{seen}");
    s.app.github.offer(7, "alice/second", &[("main", HEAD_A)]);
    let two = s.app.get("/github", Some(&s.alice.0)).await.body;
    assert!(
        two.contains(r#"class="mono">alice/bot</span>"#)
            && two.contains(r#"class="mono">alice/second</span>"#)
    );
    let empty = s
        .app
        .store
        .lock()
        .map(|st| st.replace_installations(ALICE, &[installation(55)]));
    assert!(matches!(empty, Ok(Ok(()))));
    let none = s.app.get("/github", Some(&s.alice.0)).await.body;
    assert!(
        none.contains("No repository is selected for the platform on this account"),
        "{none}"
    );
    if let Ok(mut fail) = s.app.github.fail.lock() {
        *fail = Some(platform_github::GitHubError::Unauthorized);
    }
    let broken = s.app.get("/github", Some(&s.alice.0)).await.body;
    assert!(
        broken.contains("The platform cannot read this account") && broken.contains("refused"),
        "{broken}"
    );
}

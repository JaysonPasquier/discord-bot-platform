use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_github::{parse_push, verify_signature};
use platform_store::StoredInstallation;
use serde::Deserialize;

use crate::engine::{enqueue, parse_tier};
use crate::html::{escape, AccountTab, Nav};
use crate::render::{message_back, page_in, redirect, require, server_error, with_cookies};
use crate::repos::list_repos;
use crate::session::{clear_cookie, cookie_value, set_cookie, tokens_match, STATE_TTL_SECS};
use crate::state::WebState;

const STATE_COOKIE: &str = "github_state";
const RETURN_COOKIE: &str = "github_return";

#[derive(Deserialize)]
pub(crate) struct NextQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

// Où ramener l'utilisateur après GitHub : un chemin de la plateforme, jamais une adresse reçue
// telle quelle (pas de redirection ouverte). Seuls les caractères d'un chemin simple passent.
pub(crate) fn safe_next(raw: &str) -> Option<String> {
    let simple = raw.len() <= 200
        && raw.starts_with('/')
        && !raw.starts_with("//")
        && !raw.contains("..")
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'));
    let known = raw == "/github"
        || raw == "/dashboard"
        || raw == "/integrations"
        || raw.starts_with("/products/")
        || raw.starts_with("/projects/");
    (simple && known).then(|| raw.to_string())
}

fn remember(state: &WebState, next: Option<&str>) -> Vec<String> {
    next.and_then(safe_next)
        .map(|path| {
            set_cookie(
                RETURN_COOKIE,
                &path,
                STATE_TTL_SECS,
                state.config.secure_cookies(),
            )
        })
        .into_iter()
        .collect()
}

fn callback_url(state: &WebState) -> String {
    format!("{}/github/callback", state.config.public_url)
}

// GitHub ramène le navigateur sur l'adresse du serveur (`callback_url`, jamais celle du
// frontend) : à ce stade le proxy Vite n'est plus dans la boucle. Un chemin qui n'existe que
// dans l'app React (pas une page servie par ce serveur) doit donc devenir une adresse absolue
// vers `frontend_url`, sinon le navigateur reste sur la mauvaise origine après la redirection.
fn redirect_destination(state: &WebState, path: &str) -> String {
    let spa_only = path == "/integrations" || path.starts_with("/projects/");
    match (&state.config.frontend_url, spa_only) {
        (Some(base), true) => format!("{base}{path}"),
        _ => path.to_string(),
    }
}

fn not_set_up(state: &WebState, auth: &crate::session::Auth, back: &str) -> Response {
    message_back(
        StatusCode::SERVICE_UNAVAILABLE,
        state,
        Some(auth),
        "GitHub is not set up",
        "Follow docs/SETUP_GITHUB.md, then restart the dashboard.",
        back,
    )
}

// Ce que la plateforme voit réellement pour chaque compte : c'est ce qui permet de comprendre
// pourquoi une liste de dépôts est vide.
async fn what_is_visible(state: &WebState, installation: u64) -> String {
    match list_repos(state, installation).await {
        Ok(repos) if repos.is_empty() => {
            r#"<span>No repository is selected for the platform on this account.</span>"#
                .to_string()
        }
        Ok(repos) => repos
            .iter()
            .map(|r| format!(r#"<span class="mono">{}</span>"#, escape(&r.full_name)))
            .collect::<Vec<_>>()
            .join(""),
        Err(error) => format!(
            r#"<span>The platform cannot read this account: {}</span>"#,
            escape(&error.to_string())
        ),
    }
}

pub(crate) async fn show(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let nav = Nav::Account(AccountTab::Integrations);
    if state.github.install_url().is_none() {
        let body = format!(
            r#"<div class="page narrow"><div class="page-head"><div><h1>Integrations</h1><p>Accounts connected to the platform.</p></div></div><div class="note note-warn">{}<span>GitHub is not set up on this platform yet. Follow <strong>docs/SETUP_GITHUB.md</strong>, then restart the dashboard.</span></div></div>"#,
            crate::icons::ALERT
        );
        return page_in(&auth, &state, "Integrations", &body, &nav).into_response();
    }
    let Ok(Ok(installations)) = state.store.lock().map(|s| s.installations_of(auth.user_id)) else {
        return server_error(&state, Some(&auth));
    };
    let (chip, list) = if installations.is_empty() {
        (
            r#"<span class="chip">Not connected</span>"#,
            r#"<div class="item small muted">No GitHub account connected yet. Install the app on GitHub and choose the repositories the platform may read: you come back here automatically.</div>"#.to_string(),
        )
    } else {
        // Un appel GitHub par compte connecté : en parallèle plutôt que l'un après l'autre.
        let repos_by_installation: std::collections::HashMap<u64, String> = {
            let mut set = tokio::task::JoinSet::new();
            for i in &installations {
                let state = state.clone();
                let id = i.id;
                set.spawn(async move { (id, what_is_visible(&state, id).await) });
            }
            let mut map = std::collections::HashMap::new();
            while let Some(result) = set.join_next().await {
                if let Ok((id, html)) = result {
                    map.insert(id, html);
                }
            }
            map
        };
        let mut rows = String::new();
        for i in &installations {
            let scope = if i.selection == "all" {
                "all repositories"
            } else {
                "selected repositories only"
            };
            rows.push_str(&format!(
                r#"<div class="item">{badge}<div class="grow"><div class="row"><span class="strong">{login}</span><span class="faint small">{scope}</span></div><div class="row-wrap small faint">{repos}</div></div></div>"#,
                badge = crate::icons::account_badge(i.id, "sm"),
                login = escape(&i.login),
                repos = repos_by_installation.get(&i.id).cloned().unwrap_or_default()
            ));
        }
        (r#"<span class="chip chip-ok">Connected</span>"#, rows)
    };
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Integrations</h1><p>Accounts connected to the platform.</p></div></div>
<div class="setting"><div class="s-body"><div class="row">{github}<h3 class="grow">GitHub</h3>{chip}</div><div class="card list">{list}</div></div>
<div class="s-foot"><span>The platform can read only these repositories.</span><span class="row"><a class="btn btn-sm" href="/github/connect?next=/github">Refresh</a><a class="btn btn-sm" href="/github/install?next=/github">{external} Choose repositories</a></span></div></div>
<div class="setting"><div class="s-body"><div class="row">{discord}<h3 class="grow">Discord</h3><span class="chip chip-ok">Signed in</span></div><div class="card list"><div class="item">{me}<span class="grow strong">{name}</span><span class="faint small">{servers} servers you can manage</span></div></div></div></div></div>"#,
        github = crate::icons::GITHUB,
        external = crate::icons::EXTERNAL,
        discord = crate::icons::DISCORD,
        me = crate::icons::account_badge(auth.user_id, "sm"),
        name = escape(&auth.name),
        servers = auth.guilds.len(),
    );
    page_in(&auth, &state, "Integrations", &body, &nav).into_response()
}

// Envoie l'utilisateur choisir ses dépôts sur GitHub ; la page d'installation le ramènera (Setup URL).
pub(crate) async fn install(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let Some(url) = state.github.install_url() else {
        return not_set_up(&state, &auth, "/github");
    };
    with_cookies(redirect(&url), &remember(&state, query.next.as_deref()))
}

// « Setup URL » de la GitHub App : GitHub y renvoie l'utilisateur dès qu'il a installé l'application
// ou changé ses dépôts. On enchaîne sur l'autorisation, déjà accordée donc sans écran, pour
// rafraîchir la liste des comptes puis le ramener là où il en était.
pub(crate) async fn installed(State(state): State<WebState>, headers: HeaderMap) -> Response {
    match require(&state, &headers) {
        Ok(_) => redirect("/github/connect"),
        Err(response) => response,
    }
}

pub(crate) async fn connect(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let Ok(random) = platform_store::generate_token() else {
        return server_error(&state, Some(&auth));
    };
    let Some(url) = state.github.authorize_url(&random, &callback_url(&state)) else {
        return not_set_up(&state, &auth, "/github");
    };
    let mut cookies = vec![set_cookie(
        STATE_COOKIE,
        &random,
        STATE_TTL_SECS,
        state.config.secure_cookies(),
    )];
    cookies.extend(remember(&state, query.next.as_deref()));
    with_cookies(redirect(&url), &cookies)
}

pub(crate) async fn callback(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let secure = state.config.secure_cookies();
    let cleared = [
        clear_cookie(STATE_COOKIE, secure),
        clear_cookie(RETURN_COOKIE, secure),
    ];
    let refuse = |status: StatusCode, text: &str| {
        with_cookies(
            message_back(
                status,
                &state,
                Some(&auth),
                "GitHub connection failed",
                text,
                "/github",
            ),
            &cleared,
        )
    };
    if query.error.is_some() {
        return refuse(
            StatusCode::BAD_REQUEST,
            "GitHub did not authorize the connection.",
        );
    }
    let (Some(code), Some(returned)) = (query.code, query.state) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "The answer from GitHub is incomplete.",
        );
    };
    let expected = cookie_value(&headers, STATE_COOKIE).unwrap_or_default();
    if expected.is_empty() || !tokens_match(&expected, &returned) {
        return refuse(
            StatusCode::BAD_REQUEST,
            "The connection request expired. Start again.",
        );
    }
    let github = state.github.clone();
    let redirect_uri = callback_url(&state);
    let listed =
        tokio::task::spawn_blocking(move || github.user_installations(&code, &redirect_uri)).await;
    let Ok(Ok(found)) = listed else {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "GitHub refused the code or did not answer. Try again.",
        );
    };
    let stored: Vec<StoredInstallation> = found
        .into_iter()
        .map(|i| StoredInstallation {
            id: i.id,
            login: i.login,
            selection: i.selection,
        })
        .collect();
    let saved = state
        .store
        .lock()
        .map(|s| s.replace_installations(auth.user_id, &stored));
    if !matches!(saved, Ok(Ok(()))) {
        return with_cookies(server_error(&state, Some(&auth)), &cleared);
    }
    let back = cookie_value(&headers, RETURN_COOKIE)
        .and_then(|raw| safe_next(&raw))
        .unwrap_or_else(|| "/github".to_string());
    with_cookies(redirect(&redirect_destination(&state, &back)), &cleared)
}

// GitHub prévient à chaque push. Sans secret configuré la route n'existe pas ; sans signature valide elle refuse.
pub(crate) async fn webhook(
    State(state): State<WebState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = state.config.github_webhook_secret.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !verify_signature(secret.expose().as_bytes(), &body, signature) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match headers.get("x-github-event").and_then(|v| v.to_str().ok()) {
        Some("ping") => StatusCode::OK.into_response(),
        Some("push") => {
            let Ok(Some(push)) = parse_push(&body) else {
                return StatusCode::NO_CONTENT.into_response();
            };
            let watched = state
                .store
                .lock()
                .ok()
                .and_then(|s| {
                    s.auto_deploy_sources_for(push.installation, &push.repo, &push.branch)
                        .ok()
                })
                .unwrap_or_default();
            for w in watched {
                deploy_new_head(
                    &state,
                    w.product.0,
                    &w.tier,
                    &push.branch,
                    &push.sha,
                    &w.source.last_head,
                );
            }
            StatusCode::ACCEPTED.into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

// Un commit déjà pris en compte ne se redéploie pas : un webhook rejoué ou un sondage qui le revoit ne fait rien.
fn deploy_new_head(
    state: &WebState,
    product: u64,
    tier: &str,
    branch: &str,
    head: &str,
    seen: &str,
) {
    if head == seen {
        return;
    }
    let Some(tier_kind) = parse_tier(tier) else {
        return;
    };
    let marked = state.store.lock().is_ok_and(|s| {
        s.set_last_head(platform_core::ProductId(product), tier, head)
            .is_ok()
    });
    if marked {
        let _ = enqueue(state, product, tier_kind, head, branch, "push");
    }
}

// Sondage : vérifie la tête de chaque branche suivie. C'est ce qui fait marcher l'auto-déploiement
// sur un ordinateur sans adresse publique, où GitHub ne peut pas envoyer de webhook.
pub fn poll_once(state: &WebState) {
    let watched = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.auto_deploy_sources().ok())
        .unwrap_or_default();
    for w in watched {
        let head =
            state
                .github
                .branch_head(w.source.installation, &w.source.repo, &w.source.branch);
        if let Ok(head) = head {
            deploy_new_head(
                state,
                w.product.0,
                &w.tier,
                &w.source.branch,
                &head,
                &w.source.last_head,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_simple_paths_are_accepted() {
        for ok in [
            "/github",
            "/dashboard",
            "/products/1",
            "/products/1/source/free",
            "/products/9/deployments/3",
            "/integrations",
            "/projects/1/source/free",
        ] {
            assert_eq!(safe_next(ok).as_deref(), Some(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "github",
            "//github",
            "/githubx",
            "/dashboardx",
            "/product/1",
            "/products",
            "/products/../x",
            "/products/1?a=b",
            "/products/1#x",
            "/products/1 2",
            "/products/1\\x",
            "https://evil.example",
            "/products/é",
        ] {
            assert_eq!(safe_next(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_length_limit_is_exact() {
        let at = format!("/products/{}", "1".repeat(200 - "/products/".len()));
        assert_eq!(at.len(), 200);
        assert!(safe_next(&at).is_some());
        assert!(safe_next(&format!("{at}1")).is_none());
    }
}

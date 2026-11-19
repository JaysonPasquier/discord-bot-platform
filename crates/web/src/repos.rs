use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_github::{valid_branch, valid_repo, Branch, GitHubError, Repo};
use platform_runner::tier_name;
use platform_store::{Source, StoredInstallation};
use serde::Deserialize;

use crate::engine::parse_tier;
use crate::html::{escape, ProjectTab};
use crate::products::owned_product;
use crate::render::{
    message_back, not_found, page_in, redirect, require, require_csrf, server_error,
};
use crate::session::Auth;
use crate::state::WebState;

#[derive(Deserialize)]
pub(crate) struct WizardQuery {
    installation: Option<u64>,
    repo: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SourceForm {
    csrf: String,
    installation: u64,
    repo: String,
    branch: String,
    auto_deploy: Option<String>,
    both: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct RemoveForm {
    csrf: String,
}

fn github_message(error: &GitHubError) -> (StatusCode, &'static str) {
    match error {
        GitHubError::NotFound | GitHubError::Unauthorized => (
            StatusCode::BAD_GATEWAY,
            "GitHub does not let the platform read this. Check the app is installed on this repository, then authorize GitHub again.",
        ),
        GitHubError::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "GitHub is limiting requests. Try again in a few minutes.",
        ),
        _ => (
            StatusCode::BAD_GATEWAY,
            "GitHub did not answer. Try again in a minute.",
        ),
    }
}

pub(crate) async fn list_repos(
    state: &WebState,
    installation: u64,
) -> Result<Vec<Repo>, GitHubError> {
    let github = state.github.clone();
    tokio::task::spawn_blocking(move || github.repositories(installation))
        .await
        .map_err(|_| GitHubError::Http("task failed".to_string()))?
}

pub(crate) async fn list_branches(
    state: &WebState,
    installation: u64,
    repo: &str,
) -> Result<Vec<Branch>, GitHubError> {
    let github = state.github.clone();
    let repo = repo.to_string();
    tokio::task::spawn_blocking(move || github.branches(installation, &repo))
        .await
        .map_err(|_| GitHubError::Http("task failed".to_string()))?
}

fn my_installations(state: &WebState, auth: &Auth) -> Option<Vec<StoredInstallation>> {
    state.store.lock().ok()?.installations_of(auth.user_id).ok()
}

fn back_to(id: u64) -> String {
    format!("/products/{id}")
}

// Les trois étapes de la maquette : compte, dépôt, branche.
fn steps(current: usize, account: &str, repo: &str) -> String {
    let item = |n: usize, title: &str, value: &str| {
        let done = current > n;
        let aria = if current == n {
            r#" aria-current="step""#
        } else {
            ""
        };
        let class = if done { r#" class="done""# } else { "" };
        let number = if done {
            crate::icons::CHECK.to_string()
        } else {
            n.to_string()
        };
        let text = if value.is_empty() {
            escape(title)
        } else {
            format!("{}: {}", escape(title), escape(value))
        };
        format!(
            r#"<div role="listitem"{aria}{class}><span class="n">{number}</span><span class="truncate">{text}</span></div>"#
        )
    };
    format!(
        r#"<div class="steps" role="list">{}{}{}</div>"#,
        item(1, "Account", account),
        item(2, "Repository", repo),
        item(3, "Branch", "")
    )
}

fn wizard_page(
    state: &WebState,
    auth: &Auth,
    id: u64,
    product_name: &str,
    tier: &str,
    steps_html: &str,
    body: &str,
) -> Response {
    let html = format!(
        r#"<div class="page narrow"><p class="crumb-line"><a href="/products/{id}/source">Source</a><span>/</span><span>{title}</span></p>
<div class="page-head"><div><h1>Connect a repository</h1><p>For the {tier} version. The repository needs a bot.toml at its root.</p></div></div>
{steps_html}{body}</div>"#,
        title = crate::views::tier_title(tier),
    );
    let nav = crate::project_page::nav(state, id, product_name, ProjectTab::Source);
    page_in(auth, state, "Connect a repository", &html, &nav).into_response()
}

pub(crate) async fn wizard(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, tier_raw)): Path<(u64, String)>,
    Query(query): Query<WizardQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let Some(tier) = parse_tier(&tier_raw) else {
        return not_found(&state, &auth);
    };
    let back = back_to(id);
    if state.github.install_url().is_none() {
        return message_back(
            StatusCode::SERVICE_UNAVAILABLE,
            &state,
            Some(&auth),
            "GitHub is not set up",
            "Follow docs/SETUP_GITHUB.md, then restart the dashboard.",
            &back,
        );
    }
    let Some(installations) = my_installations(&state, &auth) else {
        return server_error(&state, Some(&auth));
    };
    let here = format!("/products/{id}/source/{tier_raw}");
    let tier_str = tier_name(tier);

    let chosen = match (query.installation, installations.len()) {
        (Some(i), _) => installations.iter().find(|x| x.id == i),
        (None, 1) => installations.first(),
        _ => None,
    };
    let Some(installation) = chosen else {
        let body = if installations.is_empty() {
            format!(
                r#"<div class="card empty"><h3>No GitHub account is connected</h3><p>Install the app on GitHub and choose the repositories the platform may read: you come back here by yourself.</p><div class="row-wrap"><a class="btn btn-primary" href="/github/install?next={here}">{} Choose repositories on GitHub</a><a class="btn" href="/github/connect?next={here}">I already did</a></div></div>"#,
                crate::icons::GITHUB,
            )
        } else {
            let rows: String = installations
                .iter()
                .map(|i| {
                    let scope = if i.selection == "all" {
                        "all repositories"
                    } else {
                        "selected repositories"
                    };
                    format!(
                        r#"<div class="item">{badge}<div class="grow"><div class="strong">{login}</div><div class="faint small">{scope}</div></div><a class="btn btn-sm" href="{here}?installation={id}">Select</a></div>"#,
                        badge = crate::icons::account_badge(i.id, "sm"),
                        login = escape(&i.login),
                        id = i.id,
                    )
                })
                .collect();
            format!(
                r#"<div class="card list">{rows}<div class="item small"><a class="link" href="/github">Add a GitHub account or change access</a></div></div>"#
            )
        };
        return wizard_page(
            &state,
            &auth,
            id,
            &product.name,
            tier_str,
            &steps(1, "", ""),
            &body,
        );
    };

    let repos = match list_repos(&state, installation.id).await {
        Ok(r) => r,
        Err(error) => {
            let (status, text) = github_message(&error);
            return message_back(status, &state, Some(&auth), "GitHub", text, &back);
        }
    };
    let Some(repo_name) = query.repo.as_deref() else {
        let rows: String = repos
            .iter()
            .map(|r| {
                let chip = if r.private { "Private" } else { "Public" };
                format!(
                    r#"<div class="item">{github}<span class="grow strong truncate">{name}</span><span class="chip chip-outline">{chip}</span><a class="btn btn-sm btn-primary" href="{here}?installation={inst}&amp;repo={name}">Import</a></div>"#,
                    github = crate::icons::GITHUB,
                    name = escape(&r.full_name),
                    inst = installation.id,
                )
            })
            .collect();
        let body = if repos.is_empty() {
            format!(
                r#"<div class="card empty"><h3>No repository available</h3><p>The platform can read no repository of this account yet.</p><div class="row-wrap"><a class="btn btn-primary" href="/github/install?next={here}">Choose repositories on GitHub</a><a class="btn" href="/github/connect?next={here}">Refresh</a></div></div>"#
            )
        } else {
            let switch = if installations.len() > 1 {
                format!(
                    r#"<div class="card-foot small"><a class="link" href="{here}">Choose another account</a></div>"#
                )
            } else {
                String::new()
            };
            format!(r#"<div class="card"><div class="list">{rows}</div>{switch}</div>"#)
        };
        return wizard_page(
            &state,
            &auth,
            id,
            &product.name,
            tier_str,
            &steps(2, &installation.login, ""),
            &body,
        );
    };
    let Some(repo) = repos.iter().find(|r| r.full_name == repo_name) else {
        return message_back(
            StatusCode::NOT_FOUND,
            &state,
            Some(&auth),
            "Repository not available",
            "The platform cannot read this repository. Select it when installing the app on GitHub.",
            &back,
        );
    };
    let branches = match list_branches(&state, installation.id, &repo.full_name).await {
        Ok(b) => b,
        Err(error) => {
            let (status, text) = github_message(&error);
            return message_back(status, &state, Some(&auth), "GitHub", text, &back);
        }
    };
    let options: String = branches
        .iter()
        .map(|b| {
            let selected = if b.name == repo.default_branch {
                " selected"
            } else {
                ""
            };
            format!(
                r#"<option value="{0}"{selected}>{0}</option>"#,
                escape(&b.name)
            )
        })
        .collect();
    let both = if tier_name(tier) == "free" {
        r#"<label class="check"><input type="checkbox" name="both" value="1"><span>Use this repository for the premium version too<span class="hint">The premium start command comes from the [premium] section of its bot.toml.</span></span></label>"#
    } else {
        ""
    };
    let body = format!(
        r#"<form class="setting" method="post" action="{here}">
<div class="s-body"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="installation" value="{inst}"><input type="hidden" name="repo" value="{repo}">
<div class="row">{github}<h3 class="grow">{repo}</h3><a class="btn btn-xs" href="{here}?installation={inst}">Change</a></div>
<div class="field"><label for="src-branch">Production branch</label><select class="input" id="src-branch" name="branch">{options}</select><span class="hint">"Deploy latest" builds the newest commit of this branch.</span></div>
<label class="check"><input type="checkbox" name="auto_deploy" value="1" checked><span>Deploy every push to this branch<span class="hint">You can turn this off later in Source.</span></span></label>
{both}</div>
<div class="s-foot"><a class="link" href="/products/{id}/source">Cancel</a><button class="btn btn-primary btn-sm" type="submit">Connect repository</button></div></form>"#,
        repo = escape(&repo.full_name),
        here = here,
        csrf = escape(&auth.csrf),
        inst = installation.id,
        github = crate::icons::GITHUB,
    );
    wizard_page(
        &state,
        &auth,
        id,
        &product.name,
        tier_str,
        &steps(3, &installation.login, &repo.full_name),
        &body,
    )
}

pub(crate) async fn save(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, tier_raw)): Path<(u64, String)>,
    Form(form): Form<SourceForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    if let Err(response) = owned_product(&state, &auth, id) {
        return response;
    }
    let Some(tier) = parse_tier(&tier_raw) else {
        return not_found(&state, &auth);
    };
    let back = back_to(id);
    let owns = match state.store.lock() {
        Ok(s) => s
            .user_has_installation(auth.user_id, form.installation)
            .unwrap_or(false),
        Err(_) => return server_error(&state, Some(&auth)),
    };
    if !owns || !valid_repo(&form.repo) || !valid_branch(&form.branch) {
        return not_found(&state, &auth);
    }
    // Tout est revérifié auprès de GitHub : le formulaire ne prouve rien.
    // Les deux appels sont indépendants : en parallèle plutôt que l'un après l'autre.
    let (repos, branches) = tokio::join!(
        list_repos(&state, form.installation),
        list_branches(&state, form.installation, &form.repo)
    );
    // Un dépôt hors de l'installation répond « introuvable » : même réponse que s'il n'existait pas.
    if matches!(branches, Err(GitHubError::NotFound)) {
        return not_found(&state, &auth);
    }
    let (Ok(repos), Ok(branches)) = (repos, branches) else {
        return message_back(
            StatusCode::BAD_GATEWAY,
            &state,
            Some(&auth),
            "GitHub",
            "GitHub did not confirm this repository. Try again.",
            &back,
        );
    };
    let head = branches
        .iter()
        .find(|b| b.name == form.branch)
        .map(|b| b.sha.clone());
    let (true, Some(head)) = (repos.iter().any(|r| r.full_name == form.repo), head) else {
        return not_found(&state, &auth);
    };
    let source = Source {
        installation: form.installation,
        repo: form.repo.clone(),
        branch: form.branch.clone(),
        auto_deploy: form.auto_deploy.is_some(),
        // Activer le déploiement automatique ne déploie pas tout de suite : il attend le prochain push.
        last_head: head,
    };
    let mut tiers = vec![tier_name(tier)];
    if tier_name(tier) == "free" && form.both.is_some() {
        tiers.push("premium");
    }
    let saved = state.store.lock().map(|s| {
        tiers
            .iter()
            .try_for_each(|t| s.save_source(ProductId(id), t, &source))
    });
    if !matches!(saved, Ok(Ok(()))) {
        return server_error(&state, Some(&auth));
    }
    redirect(&back)
}

pub(crate) async fn remove(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, tier_raw)): Path<(u64, String)>,
    Form(form): Form<RemoveForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    if let Err(response) = owned_product(&state, &auth, id) {
        return response;
    }
    let Some(tier) = parse_tier(&tier_raw) else {
        return not_found(&state, &auth);
    };
    let removed = state
        .store
        .lock()
        .map(|s| s.delete_source(ProductId(id), tier_name(tier)));
    if matches!(removed, Ok(Ok(()))) {
        redirect(&back_to(id))
    } else {
        server_error(&state, Some(&auth))
    }
}

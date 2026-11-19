// Le dépôt d'une version, ses branches, les déploiements lancés depuis GitHub.
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use platform_core::ProductId;
use platform_github::{valid_branch, valid_repo, GitHubError};
use platform_runner::{container_name, tier_name};
use platform_store::Source;
use serde::{Deserialize, Serialize};

use super::auth::User;
use super::bot_actions::{find_deployment, slow_down, tier_of};
use super::bots::owned;
use super::error::ApiError;
use crate::engine::{enqueue, parse_tier};
use crate::repos::{list_branches, list_repos};
use crate::state::WebState;

fn github_error(error: &GitHubError) -> ApiError {
    match error {
        GitHubError::NotFound | GitHubError::Unauthorized => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "github_access",
            "GitHub does not let the platform read this. Check the app is installed on this repository, then authorize GitHub again.",
        ),
        GitHubError::RateLimited => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "github_limited",
            "GitHub is limiting requests. Try again in a few minutes.",
        ),
        _ => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "github_down",
            "GitHub did not answer. Try again in a minute.",
        ),
    }
}

#[derive(Serialize)]
pub(crate) struct InstallationDto {
    id: String,
    login: String,
    all_repositories: bool,
}

#[derive(Serialize)]
pub(crate) struct GithubState {
    configured: bool,
    // L'adresse du serveur, jamais celle du frontend (ils ne sont pas forcément sur la même
    // origine en local) : afficher `window.location.origin` ici montrerait l'adresse du serveur
    // Vite, pas celle qui reçoit vraiment les webhooks GitHub.
    webhook_url: String,
    installations: Vec<InstallationDto>,
}

pub(crate) async fn github(
    State(state): State<WebState>,
    User(auth): User,
) -> Result<Json<GithubState>, ApiError> {
    let installations = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .installations_of(auth.user_id)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(GithubState {
        configured: state.github.install_url().is_some(),
        webhook_url: format!("{}/github/webhook", state.config.public_url),
        installations: installations
            .into_iter()
            .map(|i| InstallationDto {
                id: i.id.to_string(),
                login: i.login,
                all_repositories: i.selection == "all",
            })
            .collect(),
    }))
}

#[derive(Serialize)]
pub(crate) struct RepoDto {
    full_name: String,
    private: bool,
    default_branch: String,
}

fn owns_installation(
    state: &WebState,
    auth: &crate::session::Auth,
    id: u64,
) -> Result<(), ApiError> {
    let owns = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .user_has_installation(auth.user_id, id)
        .unwrap_or(false);
    owns.then_some(()).ok_or_else(ApiError::not_found)
}

pub(crate) async fn repos(
    State(state): State<WebState>,
    User(auth): User,
    Path(installation): Path<u64>,
) -> Result<Json<Vec<RepoDto>>, ApiError> {
    owns_installation(&state, &auth, installation)?;
    let list = list_repos(&state, installation)
        .await
        .map_err(|e| github_error(&e))?;
    Ok(Json(
        list.into_iter()
            .map(|r| RepoDto {
                full_name: r.full_name,
                private: r.private,
                default_branch: r.default_branch,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub(crate) struct BranchQuery {
    installation: u64,
    repo: String,
}

#[derive(Serialize)]
pub(crate) struct BranchDto {
    name: String,
    sha: String,
}

pub(crate) async fn branches(
    State(state): State<WebState>,
    User(auth): User,
    Query(query): Query<BranchQuery>,
) -> Result<Json<Vec<BranchDto>>, ApiError> {
    owns_installation(&state, &auth, query.installation)?;
    if !valid_repo(&query.repo) {
        return Err(ApiError::not_found());
    }
    let list = list_branches(&state, query.installation, &query.repo)
        .await
        .map_err(|e| match e {
            GitHubError::NotFound => ApiError::not_found(),
            other => github_error(&other),
        })?;
    Ok(Json(
        list.into_iter()
            .map(|b| BranchDto {
                name: b.name,
                sha: b.sha,
            })
            .collect(),
    ))
}

fn source_of(state: &WebState, id: u64, tier: &str) -> Result<Option<Source>, ApiError> {
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .source(ProductId(id), tier)
        .map_err(|_| ApiError::internal())
}

#[derive(Deserialize)]
pub(crate) struct SourceForm {
    installation: u64,
    repo: String,
    branch: String,
    auto_deploy: bool,
    // Pour la version gratuite : utiliser le même dépôt pour la version premium.
    #[serde(default)]
    both: bool,
}

pub(crate) async fn save_source(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, tier_raw)): Path<(u64, String)>,
    Json(form): Json<SourceForm>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&tier_raw)?;
    owns_installation(&state, &auth, form.installation)?;
    if !valid_repo(&form.repo) || !valid_branch(&form.branch) {
        return Err(ApiError::not_found());
    }
    // Tout est revérifié auprès de GitHub : le formulaire ne prouve rien.
    let repos = list_repos(&state, form.installation).await;
    let branches = list_branches(&state, form.installation, &form.repo).await;
    if matches!(branches, Err(GitHubError::NotFound)) {
        return Err(ApiError::not_found());
    }
    let (Ok(repos), Ok(branches)) = (repos, branches) else {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "github_down",
            "GitHub did not confirm this repository. Try again.",
        ));
    };
    let head = branches
        .iter()
        .find(|b| b.name == form.branch)
        .map(|b| b.sha.clone());
    let (true, Some(head)) = (repos.iter().any(|r| r.full_name == form.repo), head) else {
        return Err(ApiError::not_found());
    };
    let source = Source {
        installation: form.installation,
        repo: form.repo,
        branch: form.branch,
        auto_deploy: form.auto_deploy,
        // Activer le déploiement automatique ne déploie pas tout de suite : il attend le prochain push.
        last_head: head,
    };
    let mut tiers = vec![tier_name(tier)];
    if tier_name(tier) == "free" && form.both {
        tiers.push("premium");
    }
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    tiers
        .iter()
        .try_for_each(|t| store.save_source(ProductId(id), t, &source))
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct AutoForm {
    auto_deploy: bool,
}

pub(crate) async fn set_auto(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, tier_raw)): Path<(u64, String)>,
    Json(form): Json<AutoForm>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&tier_raw)?;
    let current = source_of(&state, id, tier_name(tier))?.ok_or_else(ApiError::not_found)?;
    let mut next = Source {
        auto_deploy: form.auto_deploy,
        ..current.clone()
    };
    if form.auto_deploy && !current.auto_deploy {
        let head = list_branches(&state, current.installation, &current.repo)
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|b| b.name == current.branch))
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "github_down",
                    "The branch could not be read, so automatic deploys were not turned on. Try again in a minute.",
                )
            })?;
        next.last_head = head.sha;
    }
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .save_source(ProductId(id), tier_name(tier), &next)
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_source(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, tier_raw)): Path<(u64, String)>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&tier_raw)?;
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .delete_source(ProductId(id), tier_name(tier))
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct DeployForm {
    tier: String,
    branch: String,
}

#[derive(Serialize)]
pub(crate) struct Queued {
    id: u64,
}

pub(crate) async fn deploy_branch(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<DeployForm>,
) -> Result<(StatusCode, Json<Queued>), ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&form.tier)?;
    let source = source_of(&state, id, tier_name(tier))?.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "no_source",
            "Connect a repository to this version first.",
        )
    })?;
    slow_down(&state, &container_name(id, tier), "deploying this version")?;
    // La tête de la branche est relue chez GitHub à cet instant : on déploie le commit réel.
    let head = list_branches(&state, source.installation, &source.repo)
        .await
        .ok()
        .and_then(|list| list.into_iter().find(|b| b.name == form.branch))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "branch_unknown",
                "GitHub does not know this branch, or did not answer.",
            )
        })?;
    if head.name == source.branch {
        let _ = state
            .store
            .lock()
            .map(|s| s.set_last_head(ProductId(id), tier_name(tier), &head.sha));
    }
    let queued = enqueue(&state, id, tier, &head.sha, &head.name, "manual")
        .map_err(|_| ApiError::internal())?;
    Ok((StatusCode::ACCEPTED, Json(Queued { id: queued })))
}

pub(crate) async fn redeploy(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, deployment)): Path<(u64, u64)>,
) -> Result<(StatusCode, Json<Queued>), ApiError> {
    owned(&state, &auth, id)?;
    let old = find_deployment(&state, id, deployment)?;
    let tier = parse_tier(&old.tier).ok_or_else(ApiError::not_found)?;
    slow_down(&state, &container_name(id, tier), "deploying this version")?;
    let queued = enqueue(&state, id, tier, &old.commit, &old.branch, "redeploy")
        .map_err(|_| ApiError::internal())?;
    Ok((StatusCode::ACCEPTED, Json(Queued { id: queued })))
}

// Comme `redeploy`, mais ignore l'image déjà construite pour ce commit : utile quand le code
// source n'a pas changé de hash (même commit) mais que l'image locale est corrompue ou obsolète
// (dépendance système mise à jour, Dockerfile généré différemment).
pub(crate) async fn rebuild(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, deployment)): Path<(u64, u64)>,
) -> Result<(StatusCode, Json<Queued>), ApiError> {
    owned(&state, &auth, id)?;
    let old = find_deployment(&state, id, deployment)?;
    let tier = parse_tier(&old.tier).ok_or_else(ApiError::not_found)?;
    slow_down(&state, &container_name(id, tier), "deploying this version")?;
    let queued = enqueue(&state, id, tier, &old.commit, &old.branch, "rebuild")
        .map_err(|_| ApiError::internal())?;
    Ok((StatusCode::ACCEPTED, Json(Queued { id: queued })))
}

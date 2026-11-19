use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_runner::{container_name, tier_name};
use platform_store::{DeployStatus, Deployment, Source};
use serde::Deserialize;

use crate::engine::{enqueue, parse_tier};
use crate::html::{escape, ProjectTab};
use crate::icons;
use crate::products::owned_product;
use crate::project_page::nav;
use crate::render::{
    message_back, not_found, page_in, redirect, require, require_csrf, server_error,
};
use crate::repos::list_branches;
use crate::state::WebState;
use crate::views::{ago, short, status_badge, tier_title};

#[derive(Deserialize)]
pub(crate) struct DeployForm {
    csrf: String,
    tier: String,
    branch: String,
}

#[derive(Deserialize)]
pub(crate) struct CsrfOnly {
    csrf: String,
}

fn source_of(state: &WebState, id: u64, tier: &str) -> Option<Source> {
    state
        .store
        .lock()
        .ok()?
        .source(ProductId(id), tier)
        .ok()
        .flatten()
}

fn slow_down(state: &WebState, key: &str) -> Result<(), u32> {
    state
        .limiter
        .check_and_record(key, (state.clock)(), state.config.deploy_cooldown_secs)
}

pub(crate) async fn choose(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, tier_raw)): Path<(u64, String)>,
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
    let back = format!("/products/{id}");
    let Some(source) = source_of(&state, id, tier_name(tier)) else {
        return message_back(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "No repository connected",
            "Connect a repository to this version first.",
            &back,
        );
    };
    let branches = match list_branches(&state, source.installation, &source.repo).await {
        Ok(b) => b,
        Err(_) => {
            return message_back(
                StatusCode::BAD_GATEWAY,
                &state,
                Some(&auth),
                "GitHub did not answer",
                "The branches could not be listed. Try again in a minute.",
                &back,
            )
        }
    };
    let options: String = branches
        .iter()
        .map(|b| {
            let selected = if b.name == source.branch {
                " selected"
            } else {
                ""
            };
            format!(
                r#"<option value="{0}"{selected}>{0} ({1})</option>"#,
                escape(&b.name),
                escape(short(&b.sha))
            )
        })
        .collect();
    let html = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Deploy the {tier} version</h1><p>{repo}. The latest commit of the branch you pick is built and replaces the live version once it starts.</p></div></div>
<form class="setting" method="post" action="/products/{id}/deployments">
<div class="s-body"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="tier" value="{tier}">
<div class="field"><label for="dep-branch">Branch</label><select class="input" id="dep-branch" name="branch">{options}</select></div></div>
<div class="s-foot"><a class="link" href="{back}">Cancel</a><button class="btn btn-primary btn-sm" type="submit">Deploy this branch</button></div>
</form></div>"#,
        tier = tier_name(tier),
        repo = escape(&source.repo),
        csrf = escape(&auth.csrf),
        options = options,
        back = back,
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Deployments);
    page_in(
        &auth,
        &state,
        &format!("Deploy {}", product.name),
        &html,
        &nav,
    )
    .into_response()
}

pub(crate) async fn create(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<DeployForm>,
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
    let Some(tier) = parse_tier(&form.tier) else {
        return not_found(&state, &auth);
    };
    let back = format!("/products/{id}");
    let Some(source) = source_of(&state, id, tier_name(tier)) else {
        return message_back(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "No repository connected",
            "Connect a repository to this version first.",
            &back,
        );
    };
    if let Err(seconds) = slow_down(&state, &container_name(id, tier)) {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before deploying this version again."),
            &back,
        );
    }
    // La tête de la branche est relue chez GitHub à cet instant : on déploie le commit réel, pas celui d'un formulaire.
    let branches = list_branches(&state, source.installation, &source.repo).await;
    let head = branches
        .ok()
        .and_then(|list| list.into_iter().find(|b| b.name == form.branch));
    let Some(head) = head else {
        return message_back(
            StatusCode::BAD_GATEWAY,
            &state,
            Some(&auth),
            "Branch not found",
            "GitHub does not know this branch, or did not answer.",
            &back,
        );
    };
    if head.name == source.branch {
        let _ = state
            .store
            .lock()
            .map(|s| s.set_last_head(ProductId(id), tier_name(tier), &head.sha));
    }
    match enqueue(&state, id, tier, &head.sha, &head.name, "manual") {
        Ok(deployment) => redirect(&format!("/products/{id}/deployments/{deployment}")),
        Err(_) => server_error(&state, Some(&auth)),
    }
}

fn find(state: &WebState, id: u64, deployment: u64) -> Option<Deployment> {
    state
        .store
        .lock()
        .ok()?
        .deployment(deployment)
        .ok()
        .flatten()
        .filter(|d| d.product.0 == id)
}

pub(crate) async fn redeploy(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, deployment)): Path<(u64, u64)>,
    Form(form): Form<CsrfOnly>,
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
    let Some(old) = find(&state, id, deployment) else {
        return not_found(&state, &auth);
    };
    let Some(tier) = parse_tier(&old.tier) else {
        return not_found(&state, &auth);
    };
    let back = format!("/products/{id}/deployments/{deployment}");
    if let Err(seconds) = slow_down(&state, &container_name(id, tier)) {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before deploying this version again."),
            &back,
        );
    }
    match enqueue(&state, id, tier, &old.commit, &old.branch, "redeploy") {
        Ok(new) => redirect(&format!("/products/{id}/deployments/{new}")),
        Err(_) => message_back(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "No repository connected",
            "Connect a repository to this version first.",
            &back,
        ),
    }
}

pub(crate) async fn rebuild(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, deployment)): Path<(u64, u64)>,
    Form(form): Form<CsrfOnly>,
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
    let Some(old) = find(&state, id, deployment) else {
        return not_found(&state, &auth);
    };
    let Some(tier) = parse_tier(&old.tier) else {
        return not_found(&state, &auth);
    };
    let back = format!("/products/{id}/deployments/{deployment}");
    if let Err(seconds) = slow_down(&state, &container_name(id, tier)) {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before deploying this version again."),
            &back,
        );
    }
    match enqueue(&state, id, tier, &old.commit, &old.branch, "rebuild") {
        Ok(new) => redirect(&format!("/products/{id}/deployments/{new}")),
        Err(_) => message_back(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "No repository connected",
            "Connect a repository to this version first.",
            &back,
        ),
    }
}

// Le niveau d'une ligne de journal, pour la teinte (comme la maquette : erreur, avertissement, succès).
fn line_class(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("err!") || lower.contains("error") || lower.starts_with("build failed") {
        "error"
    } else if lower.contains("warn") {
        "warn"
    } else if lower.contains("live") || lower.contains("started") || lower.contains("success") {
        "ok"
    } else {
        ""
    }
}

fn log_lines(log: &str) -> String {
    if log.trim().is_empty() {
        return r#"<div class="logs-empty">Waiting for a build slot. This deployment starts when the current build of this version finishes.</div>"#.to_string();
    }
    log.lines()
        .enumerate()
        .map(|(i, line)| {
            format!(
                r#"<div class="ln {}"><span class="no">{}</span><span class="tx">{}</span></div>"#,
                line_class(line),
                i + 1,
                escape(line)
            )
        })
        .collect()
}

pub(crate) async fn detail(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, deployment)): Path<(u64, u64)>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let Some(d) = find(&state, id, deployment) else {
        return not_found(&state, &auth);
    };
    let now = (state.clock)();
    let source = source_of(&state, id, &d.tier);
    let credential = crate::bots::stored_credential(
        &state,
        id,
        parse_tier(&d.tier).unwrap_or(platform_runner::Tier::Free),
    )
    .ok()
    .flatten();
    let title = tier_title(&d.tier);
    let app = credential.map_or_else(
        || {
            if d.tier == "premium" {
                format!("{} Premium", product.name)
            } else {
                product.name.clone()
            }
        },
        |c| c.bot_name,
    );
    let sha = escape(short(&d.commit));
    let duration = {
        let text = crate::views::duration(&d);
        if text.is_empty() {
            "Not finished".to_string()
        } else {
            text
        }
    };
    let current = if d.status == DeployStatus::Running {
        r#"<span class="current">Current</span>"#
    } else {
        ""
    };
    let redeploy = format!(
        r#"<form method="post" action="/products/{id}/deployments/{}/redeploy"><input type="hidden" name="csrf" value="{}"><button class="btn btn-primary" type="submit"{}>{} Redeploy</button></form>
<form method="post" action="/products/{id}/deployments/{}/rebuild"><input type="hidden" name="csrf" value="{}"><button class="btn" type="submit"{} title="Force a fresh build, ignoring the cached image">{} Force rebuild</button></form>"#,
        d.id,
        escape(&auth.csrf),
        if d.status.is_active() {
            " disabled"
        } else {
            ""
        },
        icons::REDO,
        d.id,
        escape(&auth.csrf),
        if d.status.is_active() {
            " disabled"
        } else {
            ""
        },
        icons::REDO,
    );
    let repo = source.as_ref().map_or_else(
        || r#"<span class="faint">None connected</span>"#.to_string(),
        |s| {
            format!(
                r#"{}<span class="truncate">{}</span>"#,
                icons::GITHUB,
                escape(&s.repo)
            )
        },
    );
    let failed = if d.status == DeployStatus::Failed {
        let first = d
            .log
            .lines()
            .find(|l| l.starts_with("Build failed"))
            .unwrap_or("Build failed");
        let last = d
            .log
            .lines()
            .rev()
            .find(|l| line_class(l) == "error" && !l.starts_with("Build failed"))
            .unwrap_or("");
        format!(
            r#"<div class="note note-err section">{}<div><strong>{}</strong><p class="mono small">{}</p><p>The version that was live before keeps running.</p></div></div>"#,
            icons::ALERT,
            escape(first),
            escape(last)
        )
    } else {
        String::new()
    };
    let poll = if d.status.is_active() { "2" } else { "8" };
    let body = format!(
        r#"<div data-poll="{poll}"><div class="page">
<p class="crumb-line"><a href="/products/{id}/deployments">Deployments</a><span>/</span><span class="mono">{sha}</span></p>
<div data-live="detail"><div class="page-head"><div><div class="row-wrap"><h1 class="mono">{sha}</h1>{current}</div><p>{tier_lc} version, {trigger}</p></div>
<div class="row-wrap"><button class="btn" type="button" data-action="copy" data-text="{full}">{copy} Copy commit</button>{redeploy}</div></div>
<div class="dep-summary"><div class="prod-shot">{profile}</div><div class="prod-info"><dl class="kv">
<div><dt>Status</dt><dd>{state}</dd></div>
<div><dt>Duration</dt><dd>{duration}</dd></div>
<div><dt>Version</dt><dd>{title}</dd></div>
<div><dt>Started by</dt><dd class="row">{who}{trigger}, {ago}</dd></div>
<div><dt>Source</dt><dd class="row">{repo}{branch}</dd></div>
<div><dt>Image</dt><dd class="mono small">{image}</dd></div></dl></div></div>{failed}</div>
<details class="logs section" open><summary>{chev}<span>Build logs</span><span class="grow"></span><span class="row small faint">{state}<span>{duration_short}</span></span></summary>
<div class="logs-bar"><label class="input-icon" for="log-filter"><span class="sr-only">Filter log lines</span>{search}<input class="input" id="log-filter" type="search" placeholder="Find in logs" autocomplete="off"></label><label class="row small"><input class="switch" type="checkbox" id="log-follow" checked> Follow</label><span class="grow"></span><button class="btn btn-xs" type="button" data-action="copy-log">{copy} Copy</button></div>
<div data-live="log"><div class="logs-body plain" tabindex="0" aria-label="Build log" data-follow>{lines}</div></div></details>
</div></div>"#,
        tier_lc = escape(&d.tier),
        trigger = escape(&d.trigger),
        full = escape(&d.commit),
        copy = icons::COPY,
        profile = icons::dc_profile(id, &app, &format!("{title} version.")),
        state = status_badge(d.status),
        who = icons::account_badge(auth.user_id, "sm"),
        ago = ago(now, d.created_at),
        branch = crate::views::branch_pill(&d.branch),
        image = if d.image.is_empty() {
            "—".to_string()
        } else {
            escape(&d.image)
        },
        chev = icons::CHEV,
        duration_short = crate::views::duration(&d),
        search = icons::SEARCH,
        lines = log_lines(&d.log),
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Deployments);
    page_in(
        &auth,
        &state,
        &format!("{} - {}", short(&d.commit), product.name),
        &body,
        &nav,
    )
    .into_response()
}

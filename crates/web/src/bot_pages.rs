// Les onglets d'un bot : déploiements, source, Discord, réglages (même mise en page que la maquette).
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_runner::{container_name, tier_name, Tier};
use platform_store::{DeployStatus, Source};
use serde::Deserialize;

use crate::bots::stored_credential;
use crate::discord::invite_url;
use crate::engine::parse_tier;
use crate::html::{escape, ProjectTab};
use crate::icons::{self, bot_icon};
use crate::products::owned_product;
use crate::project_page::{deps_html, nav, COUNT_ROWS};
use crate::render::{
    message, message_back, not_found, page_in, redirect, require, require_csrf, server_error,
};
use crate::reports::dev_reports_section;
use crate::repos::list_branches;
use crate::state::WebState;
use crate::views::{status_label, tier_title};

#[derive(Deserialize, Default)]
pub(crate) struct ListQuery {
    tier: Option<String>,
    status: Option<String>,
    branch: Option<String>,
}

const STATUSES: [DeployStatus; 5] = [
    DeployStatus::Queued,
    DeployStatus::Building,
    DeployStatus::Running,
    DeployStatus::Ready,
    DeployStatus::Failed,
];

pub(crate) async fn deployments(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Query(query): Query<ListQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let Some(all) = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.deployments_of(ProductId(id), COUNT_ROWS).ok())
    else {
        return server_error(&state, Some(&auth));
    };
    let tier = query.tier.clone().unwrap_or_default();
    let status = query.status.clone().unwrap_or_default();
    let branch = query.branch.clone().unwrap_or_default();
    let wanted_status = DeployStatus::parse(&status);
    let shown: Vec<_> = all
        .iter()
        .filter(|d| tier.is_empty() || d.tier == tier)
        .filter(|d| status.is_empty() || Some(d.status) == wanted_status)
        .filter(|d| d.branch.contains(&branch))
        .cloned()
        .collect();
    let now = (state.clock)();
    let seg = |value: &str, label: &str| {
        format!(
            r#"<button type="submit" name="tier" value="{value}" aria-pressed="{}">{label}</button>"#,
            tier == value
        )
    };
    let options: String = STATUSES
        .iter()
        .map(|s| {
            format!(
                r#"<option value="{}"{}>{}</option>"#,
                s.as_str(),
                if status == s.as_str() {
                    " selected"
                } else {
                    ""
                },
                status_label(*s)
            )
        })
        .collect();
    let first_tier = state
        .store
        .lock()
        .ok()
        .and_then(|s| {
            ["free", "premium"]
                .into_iter()
                .find(|t| s.source(ProductId(id), t).ok().flatten().is_some())
        })
        .unwrap_or("free");
    let body = format!(
        r#"<div class="page"><div class="page-head"><div><h1>Deployments</h1><p>Every build of both versions. Redeploy any of them to go back.</p></div><a class="btn btn-primary" href="/products/{id}/deploy/{first_tier}">{branch_icon} Deploy a branch</a></div>
<form class="toolbar" method="get" action="/products/{id}/deployments"><div class="seg" role="group" aria-label="Version">{all_btn}{free_btn}{premium_btn}</div>
<label class="input-icon" for="dep-branch"><span class="sr-only">Branch</span>{branch_icon}<input class="input" id="dep-branch" type="search" name="branch" placeholder="All branches" value="{branch}" autocomplete="off"></label>
<label class="sr-only" for="dep-status">Status</label><select class="input" id="dep-status" name="status" data-autosubmit><option value="">All statuses</option>{options}</select></form>
<div class="deps"><div data-live="deploys">{rows}</div></div></div>"#,
        branch_icon = icons::BRANCH,
        all_btn = seg("", "All"),
        free_btn = seg("free", "Free"),
        premium_btn = seg("premium", "Premium"),
        branch = escape(&branch),
        rows = deps_html(
            &shown,
            &escape(&auth.csrf),
            now,
            auth.user_id,
            if all.is_empty() {
                "No deployments yet"
            } else {
                "No deployment matches these filters"
            }
        ),
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Deployments);
    page_in(
        &auth,
        &state,
        &format!("Deployments - {}", product.name),
        &body,
        &nav,
    )
    .into_response()
}

pub(crate) async fn source(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let csrf = escape(&auth.csrf);
    let sources: Vec<_> = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let mut sources = Vec::with_capacity(2);
        for tier in [Tier::Free, Tier::Premium] {
            match store.source(ProductId(id), tier_name(tier)) {
                Ok(found) => sources.push((tier, found)),
                Err(_) => return server_error(&state, Some(&auth)),
            }
        }
        sources
    };
    let mut cards = String::new();
    for (tier, found) in sources {
        let name = tier_name(tier);
        let title = tier_title(name);
        match found {
            None => cards.push_str(&format!(
                r#"<div class="setting"><div class="s-body"><h3>{title}</h3><p>No repository is connected. Choose one of the repositories you allowed on GitHub.</p></div><div class="s-foot"><span></span><a class="btn btn-primary btn-sm" href="/products/{id}/source/{name}">{} Connect repository</a></div></div>"#,
                icons::GITHUB
            )),
            Some(s) => cards.push_str(&format!(
                r#"<div class="setting"><div class="s-body"><h3>{title}</h3><div class="card list"><div class="item">{github}<span class="grow strong">{repo}</span>{branch}</div>
<form method="post" action="/products/{id}/source/{name}/auto"><input type="hidden" name="csrf" value="{csrf}"><label class="item hover"><span class="grow">Deploy every push to <span class="mono">{branch_name}</span><span class="hint">Without a public address, pushes are noticed within 30 seconds.</span></span><input class="switch" type="checkbox" name="auto_deploy" value="1" data-autosubmit{checked}><noscript><button class="btn btn-xs" type="submit">Save</button></noscript></label></form></div></div>
<div class="s-foot"><span>Changing the repository does not stop the running deployment.</span><span class="row"><form class="inline" method="post" action="/products/{id}/source/{name}/remove"><input type="hidden" name="csrf" value="{csrf}"><button class="btn btn-sm btn-danger" type="submit">Disconnect</button></form><a class="btn btn-sm" href="/products/{id}/source/{name}">Change</a></span></div></div>"#,
                github = icons::GITHUB,
                repo = escape(&s.repo),
                branch = crate::views::branch_pill(&s.branch),
                branch_name = escape(&s.branch),
                checked = if s.auto_deploy { " checked" } else { "" },
            )),
        }
    }
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Source</h1><p>Each version builds from a repository and a branch. Both can use the same repository.</p></div></div>{cards}
<div class="note section">{}<span>The platform reads only the repositories you selected when installing the GitHub app. <a class="link" href="/github">Manage access</a></span></div></div>"#,
        icons::INFO
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Source);
    page_in(
        &auth,
        &state,
        &format!("Source - {}", product.name),
        &body,
        &nav,
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct AutoForm {
    csrf: String,
    auto_deploy: Option<String>,
}

// Activer le déploiement automatique ne déploie pas tout de suite : on note la tête actuelle de la
// branche, et seul le prochain push déclenche un déploiement.
pub(crate) async fn toggle_auto(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, tier_raw)): Path<(u64, String)>,
    Form(form): Form<AutoForm>,
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
    let back = format!("/products/{id}/source");
    let current = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.source(ProductId(id), tier_name(tier)).ok().flatten());
    let Some(current) = current else {
        return not_found(&state, &auth);
    };
    let wanted = form.auto_deploy.is_some();
    let mut next = Source {
        auto_deploy: wanted,
        ..current.clone()
    };
    if wanted && !current.auto_deploy {
        let head = list_branches(&state, current.installation, &current.repo)
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|b| b.name == current.branch));
        let Some(head) = head else {
            return message_back(
                StatusCode::BAD_GATEWAY,
                &state,
                Some(&auth),
                "GitHub did not answer",
                "The branch could not be read, so automatic deploys were not turned on. Try again in a minute.",
                &back,
            );
        };
        next.last_head = head.sha;
    }
    let saved = state
        .store
        .lock()
        .map(|s| s.save_source(ProductId(id), tier_name(tier), &next));
    if matches!(saved, Ok(Ok(()))) {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

pub(crate) async fn discord(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let csrf = escape(&auth.csrf);
    let mut forms = String::new();
    for tier in [Tier::Free, Tier::Premium] {
        let name = tier_name(tier);
        let credential = match stored_credential(&state, id, tier) {
            Ok(c) => c,
            Err(_) => return server_error(&state, Some(&auth)),
        };
        let (chip, app, label) = match &credential {
            Some(c) => (
                r#"<span class="chip chip-ok">Token verified</span>"#,
                format!(
                    r#"<div class="card list"><div class="item">{}<div class="grow"><div class="strong">{}</div><div class="faint mono small">{}</div></div><a class="btn btn-sm" href="{}">{} Invite link</a></div></div>"#,
                    bot_icon(id, 32),
                    escape(&c.bot_name),
                    escape(&c.client_id),
                    escape(&invite_url(&c.client_id)),
                    icons::DISCORD
                ),
                "Replace the bot token",
            ),
            None => (
                r#"<span class="chip">No token</span>"#,
                String::new(),
                "Bot token",
            ),
        };
        forms.push_str(&format!(
            r#"<form class="setting" method="post" action="/products/{id}/bot"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="tier" value="{name}">
<div class="s-body"><div class="row"><h3 class="grow">{title}</h3>{chip}</div>{app}
<div class="field"><label for="tok-{name}">{label}</label><input class="input mono" id="tok-{name}" type="password" name="token" autocomplete="off" maxlength="200" required placeholder="Paste the token from the Bot tab"></div></div>
<div class="s-foot"><span>Checked with Discord, then stored encrypted. It is never shown again.</span><button class="btn btn-sm" type="submit">Save token</button></div></form>"#,
            title = tier_title(name)
        ));
    }
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Discord</h1><p>Each version is its own Discord application. Server owners invite these applications.</p></div></div>{forms}
<p class="hint section">A version that is already running keeps its old token until you deploy again.</p></div>"#
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Discord);
    page_in(
        &auth,
        &state,
        &format!("Discord - {}", product.name),
        &body,
        &nav,
    )
    .into_response()
}

pub(crate) async fn settings(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let (csrf, name) = (escape(&auth.csrf), escape(&product.name));
    let now = (state.clock)();
    let reports = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.reports_for_product(ProductId(id)).ok())
        .unwrap_or_default();
    let reports_html = dev_reports_section(&reports, id, &auth.csrf, now);
    let current_webhook = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.notification_webhook(id).ok().flatten())
        .unwrap_or_default();
    let webhook_placeholder = escape(&current_webhook);
    let webhook_section = format!(
        r#"<form class="setting" method="post" action="/products/{id}/webhook"><input type="hidden" name="csrf" value="{csrf}">
<div class="s-body"><h3>Deployment notifications</h3><p>Paste a Discord webhook URL to receive a message each time a deployment succeeds or fails.</p>
<div class="field"><label for="set-webhook">Discord webhook URL</label><input class="input" id="set-webhook" name="webhook_url" type="url" value="{webhook_placeholder}" placeholder="https://discord.com/api/webhooks/..." maxlength="512" autocomplete="off"></div></div>
<div class="s-foot"><span>Leave empty to disable. Tested on save.</span><button class="btn btn-sm" type="submit">Save</button></div></form>"#
    );
    let cede_form = if product.ceded {
        r#"<div class="setting"><div class="s-body"><h3>Bot ceded to BotPlatform</h3><p class="faint">This bot was ceded to BotPlatform and is no longer yours to manage.</p></div></div>"#.to_string()
    } else {
        format!(
            r#"<form class="setting danger" method="post" action="/products/{id}/cede"><input type="hidden" name="csrf" value="{csrf}">
<div class="s-body"><h3>Cede this bot to BotPlatform</h3><p>Transfer ownership to BotPlatform instead of deleting. Existing subscribers are not disrupted. This cannot be undone.</p></div>
<div class="s-foot"><span>Your name is removed from the listing.</span><button class="btn btn-sm btn-danger" type="submit">Cede bot</button></div></form>"#
        )
    };
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Settings</h1></div></div>
<form class="setting" method="post" action="/products/{id}/settings"><input type="hidden" name="csrf" value="{csrf}">
<div class="s-body"><h3>Bot name</h3><p>Shown on the marketplace and in your dashboard.</p><label class="sr-only" for="set-name">Bot name</label><input class="input" id="set-name" name="name" value="{name}" minlength="3" maxlength="32" required></div>
<div class="s-foot"><span>3 to 32 characters.</span><button class="btn btn-sm" type="submit">Save</button></div></form>
{webhook_section}
{reports_html}
<form class="setting danger" method="post" action="/products/{id}/stop-all"><input type="hidden" name="csrf" value="{csrf}">
<div class="s-body"><h3>Stop both versions</h3><p>Containers stop. Subscribers keep their subscription until you delete the bot.</p></div>
<div class="s-foot"><span></span><button class="btn btn-sm btn-danger" type="submit">Stop bot</button></div></form>
{cede_form}
<form class="setting danger" method="post" action="/products/{id}/delete"><input type="hidden" name="csrf" value="{csrf}">
<div class="s-body"><h3>Delete this bot</h3><p>Removes the listing, the deployments and the stored tokens. This cannot be undone.</p><div class="field"><label for="confirm-name">Type <strong>{name}</strong> to confirm</label><input class="input" id="confirm-name" name="confirm" autocomplete="off" required></div></div>
<div class="s-foot"><span></span><button class="btn btn-sm btn-danger" type="submit">Delete bot</button></div></form></div>"#
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Settings);
    page_in(
        &auth,
        &state,
        &format!("Settings - {}", product.name),
        &body,
        &nav,
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct RenameForm {
    csrf: String,
    name: String,
}

pub(crate) async fn rename(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<RenameForm>,
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
    let back = format!("/products/{id}/settings");
    if !platform_config::valid_name(&form.name) {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid name",
            "A bot name has 3 to 32 characters: letters, digits, spaces, - and _.",
            &back,
        );
    }
    let saved = state
        .store
        .lock()
        .map(|s| s.rename_product(ProductId(id), &form.name));
    if matches!(saved, Ok(Ok(()))) {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

#[derive(Deserialize)]
pub(crate) struct CsrfOnly {
    csrf: String,
}

async fn stop_both(state: &WebState, id: u64) {
    for tier in [Tier::Free, Tier::Premium] {
        let name = container_name(id, tier);
        let containers = state.containers.clone();
        let _ = tokio::task::spawn_blocking(move || containers.stop(&name)).await;
    }
}

pub(crate) async fn stop_all(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
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
    stop_both(&state, id).await;
    redirect(&format!("/products/{id}"))
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    csrf: String,
    confirm: String,
}

// Irréversible : le nom du bot doit être retapé, comme dans la maquette.
pub(crate) async fn delete(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    if form.confirm != product.name {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Nothing was deleted",
            "The name you typed does not match this bot.",
            &format!("/products/{id}/settings"),
        );
    }
    stop_both(&state, id).await;
    let deleted = state.store.lock().map(|s| s.delete_product(ProductId(id)));
    if matches!(deleted, Ok(Ok(()))) {
        redirect("/dashboard")
    } else {
        message(
            StatusCode::INTERNAL_SERVER_ERROR,
            &state,
            Some(&auth),
            "Something went wrong",
            "The bot could not be deleted.",
        )
    }
}

pub(crate) async fn cede(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<CsrfOnly>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    if product.ceded {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Already ceded",
            "This bot has already been ceded to BotPlatform.",
            &format!("/products/{id}/settings"),
        );
    }
    let ceded = state
        .store
        .lock()
        .map(|s| s.cede_product(ProductId(id), auth.user_id));
    if matches!(ceded, Ok(Ok(true))) {
        redirect("/dashboard")
    } else {
        server_error(&state, Some(&auth))
    }
}

#[derive(Deserialize)]
pub(crate) struct WebhookForm {
    csrf: String,
    webhook_url: String,
}

pub(crate) async fn save_webhook(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<WebhookForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if let Err(r) = owned_product(&state, &auth, id) {
        return r;
    }
    let back = format!("/products/{id}/settings");
    let url = form.webhook_url.trim();
    let result = if url.is_empty() {
        state
            .store
            .lock()
            .map(|s| s.delete_notification_webhook(id))
    } else {
        if !url.starts_with("https://discord.com/api/webhooks/")
            && !url.starts_with("https://discordapp.com/api/webhooks/")
        {
            return message_back(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Invalid URL",
                "Only Discord webhook URLs are accepted (https://discord.com/api/webhooks/...).",
                &back,
            );
        }
        state
            .store
            .lock()
            .map(|s| s.set_notification_webhook(id, url))
    };
    if matches!(result, Ok(Ok(()))) {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

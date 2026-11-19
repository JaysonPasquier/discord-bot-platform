// Les pages du compte : créer un bot, et les serveurs (abonnements premium).
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use platform_core::PremiumStatus;
use serde::Deserialize;

use crate::billing::{load_entitlement, status_of};
use crate::dashboard::guild_name;
use crate::html::{date, escape, AccountTab, Nav};
use crate::icons::{self, bot_icon};
use crate::render::{message_back, page_in, require, require_csrf, server_error};
use crate::repos::list_repos;
use crate::state::WebState;

// Les dépôts que la plateforme peut lire, pour savoir quoi importer ; ils se choisissent sur GitHub.
async fn repository_rows(state: &WebState, user: u64) -> String {
    if state.github.install_url().is_none() {
        return r#"<div class="item small muted">GitHub is not set up on this platform yet. Follow docs/SETUP_GITHUB.md, then restart.</div>"#.to_string();
    }
    let Some(installations) = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.installations_of(user).ok())
    else {
        return String::new();
    };
    let mut rows = String::new();
    for i in &installations {
        if let Ok(repos) = list_repos(state, i.id).await {
            for r in repos {
                rows.push_str(&format!(
                    r#"<div class="item">{}<span class="grow strong">{}</span><span class="faint small">{}</span></div>"#,
                    icons::GITHUB,
                    escape(&r.full_name),
                    escape(&i.login)
                ));
            }
        }
    }
    if rows.is_empty() {
        r#"<div class="item small muted">No repository yet. Choose repositories on GitHub from the Integrations tab.</div>"#.to_string()
    } else {
        rows
    }
}

pub(crate) async fn new_bot(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let rows = repository_rows(&state, auth.user_id).await;
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Create a bot</h1><p>Name it, then connect its repository and its Discord applications.</p></div></div>
<form class="setting" method="post" action="/products"><div class="s-body"><input type="hidden" name="csrf" value="{csrf}"><h3>Name</h3>
<div class="field"><label class="sr-only" for="new-name">Name</label><input class="input" id="new-name" name="name" required minlength="3" maxlength="32" placeholder="My bot" autocomplete="off"></div></div>
<div class="s-foot"><span>3 to 32 characters. Shown on the marketplace.</span><button class="btn btn-primary btn-sm" type="submit">Create bot</button></div></form>
<div class="section"><div class="section-head"><h2>Repositories you can import</h2><a class="link small" href="/github">Manage GitHub access</a></div>
<div class="card list">{rows}</div><p class="hint section">You choose the repository of each version after creating the bot.</p></div></div>"#,
        csrf = escape(&auth.csrf),
    );
    page_in(
        &auth,
        &state,
        "Create a bot",
        &body,
        &Nav::Account(AccountTab::Bots),
    )
    .into_response()
}

pub(crate) async fn servers(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let Ok(store) = state.store.lock() else {
        return server_error(&state, Some(&auth));
    };
    let Ok(subscriptions) = store.subscriptions_of(auth.user_id) else {
        return server_error(&state, Some(&auth));
    };
    let csrf = escape(&auth.csrf);
    let mut rows = String::new();
    for sub in subscriptions {
        let (Ok(Some(product)), Ok(entitlement)) = (
            store.product(sub.product),
            load_entitlement(&store, sub.guild, sub.product),
        ) else {
            return server_error(&state, Some(&auth));
        };
        let nick = store
            .guild_profile(sub.guild, sub.product)
            .ok()
            .flatten()
            .and_then(|p| p.nick)
            .map_or_else(String::new, |n| {
                format!(r#"<span class="faint small"> as {}</span>"#, escape(&n))
            });
        let status = status_of(&state, &entitlement);
        let until = if entitlement.paid_until == i64::MIN {
            "never".to_string()
        } else {
            date(entitlement.paid_until)
        };
        let (chip, billing) = match status {
            PremiumStatus::Active => (
                r#"<span class="chip chip-ok">Active</span>"#,
                format!("Paid until {until}"),
            ),
            PremiumStatus::Grace => (
                r#"<span class="chip chip-warn">Ending soon</span>"#,
                format!("Ends {until}"),
            ),
            PremiumStatus::Expired => (
                r#"<span class="chip">Expired</span>"#,
                format!("Ended {until}"),
            ),
        };
        let actions = if status == PremiumStatus::Expired || entitlement.canceled {
            format!(
                r#"<a class="btn btn-xs" href="/bots/{}">Subscribe again</a>"#,
                sub.product.0
            )
        } else {
            format!(
                r#"<span class="row"><a class="btn btn-xs" href="/subscriptions/{p}/{g}">Customize</a><form class="inline" method="post" action="/subscriptions/cancel"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="product" value="{p}"><input type="hidden" name="guild" value="{g}"><button class="btn btn-xs btn-ghost btn-danger" type="submit">Cancel</button></form></span>"#,
                p = sub.product.0,
                g = sub.guild.0
            )
        };
        rows.push_str(&format!(
            r#"<tr><td><div class="row">{badge}<span class="strong">{server}</span></div></td>
<td><a class="row" href="/bots/{p}">{icon}<span><span class="strong">{bot}</span>{nick}</span></a></td>
<td>{days}-day</td><td>{chip}</td><td class="nowrap muted">{billing}</td><td class="actions">{actions}</td></tr>"#,
            badge = icons::account_badge(sub.guild.0, "sm"),
            server = escape(&guild_name(&auth, sub.guild)),
            p = sub.product.0,
            icon = bot_icon(sub.product.0, 24),
            bot = escape(&product.name),
            days = state.config.subscription_days,
        ));
    }
    drop(store);
    let table = if rows.is_empty() {
        r#"<div class="card empty"><h3>No subscriptions</h3><p>Subscribe to premium from a bot's page.</p><a class="btn" href="/market">Find a bot</a></div>"#.to_string()
    } else {
        format!(
            r#"<div class="table-wrap"><table class="subs-table"><thead><tr><th>Server</th><th>Bot</th><th>Plan</th><th>Status</th><th>Billing</th><th><span class="sr-only">Actions</span></th></tr></thead><tbody>{rows}</tbody></table></div>"#
        )
    };
    let body = format!(
        r#"<div class="page"><div class="page-head"><div><h1>Servers</h1><p>Premium subscriptions for the servers you manage. Each one applies to one server.</p></div><a class="btn btn-primary" href="/market">Find a bot</a></div>{table}</div>"#
    );
    page_in(
        &auth,
        &state,
        "Servers",
        &body,
        &Nav::Account(AccountTab::Servers),
    )
    .into_response()
}

pub(crate) async fn delete_confirm(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let csrf = escape(&auth.csrf);
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Delete account</h1><p>This action is permanent and cannot be undone.</p></div></div>
<div class="card card-pad">
<p>Deleting your account will:</p>
<ul><li>Cancel all your subscriptions</li><li>Remove all your sessions</li><li>Delete your user record from the platform</li></ul>
<p class="hint">Your bots and their settings are <strong>not</strong> deleted. Contact support if you need to remove them too.</p>
<form method="post" action="/account/delete">
<input type="hidden" name="csrf" value="{csrf}">
<button class="btn btn-danger btn-block" type="submit">Delete my account permanently</button>
</form>
<a class="btn btn-block btn-ghost" href="/dashboard">Cancel</a>
</div></div>"#
    );
    page_in(
        &auth,
        &state,
        "Delete account",
        &body,
        &Nav::Account(AccountTab::Bots),
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    csrf: String,
}

pub(crate) async fn delete_account(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let deleted = state.store.lock().map(|s| s.delete_account(auth.user_id));
    if matches!(deleted, Ok(Ok(()))) {
        message_back(
            axum::http::StatusCode::OK,
            &state,
            None,
            "Account deleted",
            "Your account has been deleted. You can close this tab.",
            "/",
        )
    } else {
        server_error(&state, Some(&auth))
    }
}

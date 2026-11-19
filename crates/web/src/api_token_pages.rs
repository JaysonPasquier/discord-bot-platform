// Gestion des tokens API personnels (Bearer tokens pour l'API REST).
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::html::{date, escape, AccountTab, Nav};
use crate::render::{page_in, redirect, require, require_csrf, server_error};
use crate::state::WebState;

const MAX_TOKEN_NAME: usize = 64;

pub(crate) async fn list(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let tokens = match state.store.lock().map(|s| s.list_api_tokens(auth.user_id)) {
        Ok(Ok(t)) => t,
        _ => return server_error(&state, Some(&auth)),
    };
    let csrf = escape(&auth.csrf);
    let rows: String = if tokens.is_empty() {
        r#"<tr><td colspan="3" class="faint">No tokens yet.</td></tr>"#.to_string()
    } else {
        tokens
            .iter()
            .map(|t| {
                format!(
                    r#"<tr>
<td class="strong">{name}</td>
<td class="mono faint">{prefix}…</td>
<td class="nowrap muted">{created}</td>
<td class="actions"><form class="inline" method="post" action="/account/tokens/{id}/delete">
<input type="hidden" name="csrf" value="{csrf}">
<button class="btn btn-xs btn-danger" type="submit">Revoke</button>
</form></td></tr>"#,
                    name = escape(&t.name),
                    prefix = escape(&t.prefix),
                    created = date(t.created_at),
                    id = t.id,
                )
            })
            .collect()
    };
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>API Tokens</h1>
<p>Bearer tokens for the REST API. Each token has the same access as your account. Revoke tokens you no longer need.</p></div></div>
<div class="setting"><div class="s-body"><h3>Your tokens</h3>
<div class="table-wrap"><table><thead><tr><th>Name</th><th>Prefix</th><th>Created</th><th></th></tr></thead><tbody>{rows}</tbody></table></div>
</div></div>
<div class="setting"><div class="s-body"><h3>Create a token</h3>
<form id="new-token-form" method="post" action="/account/tokens">
<input type="hidden" name="csrf" value="{csrf}">
<div class="field"><label for="token-name">Name</label>
<input class="input" id="token-name" name="name" required maxlength="{MAX_TOKEN_NAME}" placeholder="CI deploy" autocomplete="off">
<span class="hint">Describe what this token is for.</span></div>
</div></div>
<div class="s-foot"><span></span><button class="btn btn-primary btn-sm" form="new-token-form" type="submit">Create token</button></div>
</div></div>"#
    );
    page_in(
        &auth,
        &state,
        "API Tokens",
        &body,
        &Nav::Account(AccountTab::ApiTokens),
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct CreateForm {
    csrf: String,
    name: String,
}

pub(crate) async fn create(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let name = form.name.trim();
    if name.is_empty() || name.len() > MAX_TOKEN_NAME {
        return redirect("/account/tokens");
    }
    let now = (state.clock)();
    let token = match state
        .store
        .lock()
        .map(|s| s.create_api_token(auth.user_id, name, now))
    {
        Ok(Ok(t)) => t,
        _ => return server_error(&state, Some(&auth)),
    };
    let token_escaped = escape(&token);
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Token created</h1>
<p>Copy this token now — it will not be shown again.</p></div></div>
<div class="card card-pad">
<p class="strong">Your new API token:</p>
<pre class="mono token-display">{token_escaped}</pre>
<p class="hint">Store it in a secrets manager or environment variable. Anyone who has this token can act on your behalf.</p>
<a class="btn btn-primary" href="/account/tokens">Done</a>
</div></div>"#
    );
    page_in(
        &auth,
        &state,
        "Token created",
        &body,
        &Nav::Account(AccountTab::ApiTokens),
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    csrf: String,
}

pub(crate) async fn delete(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let deleted = state
        .store
        .lock()
        .map(|s| s.delete_api_token(id, auth.user_id));
    if matches!(deleted, Ok(Ok(()))) {
        redirect("/account/tokens")
    } else {
        server_error(&state, Some(&auth))
    }
}

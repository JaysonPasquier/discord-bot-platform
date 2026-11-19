// Variables d'environnement par bot et par tier : liste, ajout, suppression.
// Les valeurs sont chiffrées avec ChaCha20Poly1305 (vault) avant stockage.
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_runner::{tier_name, Tier};
use platform_store::{
    env_key_ok, hex_decode, to_hex, EnvVar, MAX_ENV_VARS, MAX_KEY_LEN, MAX_VALUE_LEN,
};
use serde::Deserialize;

use crate::html::{escape, ProjectTab};
use crate::products::owned_product;
use crate::project_page::nav;
use crate::render::{
    message_back, not_found, page_in, redirect, require, require_csrf, server_error,
};
use crate::state::WebState;
use crate::views::tier_title;

fn encrypt_env(
    vault: &platform_store::Vault,
    product: u64,
    tier: &str,
    key: &str,
    value: &str,
) -> String {
    let ctx = format!("env:{product}:{tier}:{key}");
    match vault.seal(value, &ctx) {
        Ok(blob) => format!("ENC:{}", to_hex(&blob)),
        Err(_) => value.to_string(),
    }
}

pub(crate) fn decrypt_env(
    vault: &platform_store::Vault,
    product: u64,
    tier: &str,
    key: &str,
    value: &str,
) -> String {
    let Some(hex) = value.strip_prefix("ENC:") else {
        return value.to_string();
    };
    let Some(bytes) = hex_decode(hex) else {
        return value.to_string();
    };
    let ctx = format!("env:{product}:{tier}:{key}");
    vault
        .open(&bytes, &ctx)
        .unwrap_or_else(|_| value.to_string())
}

pub(crate) async fn show(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let csrf = escape(&auth.csrf);
    let mut sections = String::new();
    for tier in [Tier::Free, Tier::Premium] {
        let name = tier_name(tier);
        let title = tier_title(name);
        let vars = match state.store.lock().map(|s| s.env_vars(id, name)) {
            Ok(Ok(v)) => v,
            _ => return server_error(&state, Some(&auth)),
        };
        let count = vars.len();
        sections.push_str(&section_html(id, name, title, &vars, &csrf, count));
    }
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Environment variables</h1><p>Injected into the container at each deployment. Up to {MAX_ENV_VARS} per version. Values are stored in the local database and never shown after saving.</p></div></div>{sections}</div>"#
    );
    let n = nav(&state, id, &product.name, ProjectTab::Env);
    page_in(
        &auth,
        &state,
        &format!("Env vars — {}", product.name),
        &body,
        &n,
    )
    .into_response()
}

fn section_html(
    id: u64,
    tier: &str,
    title: &str,
    vars: &[EnvVar],
    csrf: &str,
    count: usize,
) -> String {
    let rows: String = if vars.is_empty() {
        format!(
            r#"<tr class="env-empty"><td colspan="2">No variables for the {title} version yet.</td></tr>"#
        )
    } else {
        vars.iter()
            .map(|v| {
                format!(
                    r#"<tr>
<td class="env-key mono">{key}</td>
<td class="env-actions">
  <span class="env-mask">••••••••</span>
  <form class="inline" method="post" action="/products/{id}/env/delete">
    <input type="hidden" name="csrf" value="{csrf}">
    <input type="hidden" name="tier" value="{tier}">
    <input type="hidden" name="key" value="{key}">
    <button class="btn btn-xs btn-danger" type="submit">Remove</button>
  </form>
</td>
</tr>"#,
                    key = escape(&v.key),
                )
            })
            .collect()
    };
    let can_add = count < MAX_ENV_VARS;
    let add_form = if can_add {
        format!(
            r#"<form class="env-add-form" method="post" action="/products/{id}/env">
<input type="hidden" name="csrf" value="{csrf}">
<input type="hidden" name="tier" value="{tier}">
<input class="input mono" name="key" placeholder="VARIABLE_NAME" maxlength="{MAX_KEY_LEN}" required autocomplete="off" spellcheck="false">
<input class="input" name="value" placeholder="value" maxlength="{MAX_VALUE_LEN}" required autocomplete="off" spellcheck="false">
<button class="btn btn-sm" type="submit">Add</button>
</form>"#
        )
    } else {
        format!(r#"<p class="hint">Maximum of {MAX_ENV_VARS} variables reached.</p>"#)
    };
    format!(
        r#"<div class="setting"><div class="s-body"><h3>{title}<span class="chip" aria-label="{count} variables">{count}</span></h3>
<table class="env-table"><thead><tr><th>Key</th><th></th></tr></thead><tbody>{rows}</tbody></table>
{add_form}</div></div>"#
    )
}

#[derive(Deserialize)]
pub(crate) struct AddForm {
    csrf: String,
    tier: String,
    key: String,
    value: String,
}

pub(crate) async fn add(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<AddForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if owned_product(&state, &auth, id).is_err() {
        return not_found(&state, &auth);
    }
    let back = format!("/products/{id}/env");
    if form.tier != "free" && form.tier != "premium" {
        return not_found(&state, &auth);
    }
    if !env_key_ok(&form.key) {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid key",
            "Keys start with a letter or underscore, then letters, digits and underscores only.",
            &back,
        );
    }
    if form.value.len() > MAX_VALUE_LEN {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Value too long",
            &format!("Maximum {MAX_VALUE_LEN} characters."),
            &back,
        );
    }
    let count = match state.store.lock().map(|s| s.count_env_vars(id, &form.tier)) {
        Ok(Ok(n)) => n,
        _ => return server_error(&state, Some(&auth)),
    };
    if count >= MAX_ENV_VARS {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Limit reached",
            &format!("Maximum of {MAX_ENV_VARS} variables per version."),
            &back,
        );
    }
    let encrypted = encrypt_env(&state.vault, id, &form.tier, &form.key, &form.value);
    let saved = state
        .store
        .lock()
        .map(|s| s.set_env_var(id, &form.tier, &form.key, &encrypted));
    if matches!(saved, Ok(Ok(()))) {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    csrf: String,
    tier: String,
    key: String,
}

pub(crate) async fn delete(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if owned_product(&state, &auth, id).is_err() {
        return not_found(&state, &auth);
    }
    let deleted = state
        .store
        .lock()
        .map(|s| s.delete_env_var(id, &form.tier, &form.key));
    if matches!(deleted, Ok(Ok(_))) {
        redirect(&format!("/products/{id}/env"))
    } else {
        server_error(&state, Some(&auth))
    }
}

// Gestion des plans tarifaires par le créateur du bot (UI HTML).
// Routes : GET /products/{id}/listing    — affiche les plans + formulaire d'ajout
//          POST /products/{id}/listing   — crée un plan (name, price_cents, days)
//          POST /products/{id}/listing/{plan_id}/delete — supprime un plan
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_store::Plan;
use serde::Deserialize;

use crate::html::{escape, ProjectTab};
use crate::products::owned_product;
use crate::project_page::nav;
use crate::render::{message_back, page_in, redirect, require, require_csrf, server_error};
use crate::state::WebState;

#[derive(Deserialize)]
pub(crate) struct AddPlanForm {
    name: String,
    price_cents: String,
    days: String,
    csrf: String,
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    csrf: String,
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
    let plans = match state
        .store
        .lock()
        .map(|s| s.plans_of(platform_core::ProductId(id)))
    {
        Ok(Ok(p)) => p,
        _ => return server_error(&state, Some(&auth)),
    };
    let csrf = escape(&auth.csrf);
    let body = build_page(id, &product.name, &plans, &csrf, None);
    let n = nav(&state, id, &product.name, ProjectTab::Listing);
    page_in(
        &auth,
        &state,
        &format!("Plans — {}", product.name),
        &body,
        &n,
    )
    .into_response()
}

pub(crate) async fn create(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<AddPlanForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if let Err(r) = owned_product(&state, &auth, id).map(|_| ()) {
        return r;
    }
    let name = form.name.trim().to_string();
    if name.is_empty() || name.len() > 80 {
        return error_page(&state, &auth, id, "Plan name must be 1–80 characters.");
    }
    let Ok(price_cents) = form.price_cents.trim().parse::<u32>() else {
        return error_page(&state, &auth, id, "Price must be a whole number of cents.");
    };
    let Ok(days) = form.days.trim().parse::<u32>() else {
        return error_page(
            &state,
            &auth,
            id,
            "Duration must be a whole number of days (1–3650).",
        );
    };
    if !(1..=3650).contains(&days) {
        return error_page(
            &state,
            &auth,
            id,
            "Duration must be between 1 and 3650 days.",
        );
    }
    // Charge tous les plans existants, ajoute le nouveau, enregistre.
    let mut plans = match state
        .store
        .lock()
        .map(|s| s.plans_of(platform_core::ProductId(id)))
    {
        Ok(Ok(p)) => p,
        _ => return server_error(&state, Some(&auth)),
    };
    plans.push(Plan {
        id: 0, // la DB attribue l'identifiant
        name,
        price_cents,
        days,
    });
    if state
        .store
        .lock()
        .map_err(|_| ())
        .and_then(|s| {
            s.replace_plans(platform_core::ProductId(id), &plans)
                .map_err(|_| ())
        })
        .is_err()
    {
        return server_error(&state, Some(&auth));
    }
    redirect(&format!("/products/{id}/listing"))
}

pub(crate) async fn delete_plan(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, plan_id)): Path<(u64, u64)>,
    Form(form): Form<DeleteForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if let Err(r) = owned_product(&state, &auth, id).map(|_| ()) {
        return r;
    }
    let plans = match state
        .store
        .lock()
        .map(|s| s.plans_of(platform_core::ProductId(id)))
    {
        Ok(Ok(p)) => p,
        _ => return server_error(&state, Some(&auth)),
    };
    let new_plans: Vec<Plan> = plans.into_iter().filter(|p| p.id != plan_id).collect();
    if state
        .store
        .lock()
        .map_err(|_| ())
        .and_then(|s| {
            s.replace_plans(platform_core::ProductId(id), &new_plans)
                .map_err(|_| ())
        })
        .is_err()
    {
        return server_error(&state, Some(&auth));
    }
    redirect(&format!("/products/{id}/listing"))
}

fn error_page(state: &WebState, auth: &crate::session::Auth, id: u64, msg: &str) -> Response {
    message_back(
        StatusCode::BAD_REQUEST,
        state,
        Some(auth),
        "Invalid plan",
        msg,
        &format!("/products/{id}/listing"),
    )
}

fn build_page(id: u64, name: &str, plans: &[Plan], csrf: &str, _error: Option<&str>) -> String {
    let rows = if plans.is_empty() {
        r#"<tr><td colspan="4" class="empty">No plans yet — add one below.</td></tr>"#.to_string()
    } else {
        plans.iter().map(|p| {
            let price = if p.price_cents == 0 {
                "Free".to_string()
            } else {
                format!("${:.2}", f64::from(p.price_cents) / 100.0)
            };
            format!(
                r#"<tr><td>{}</td><td>{price}</td><td>{} days</td><td><form method="post" action="/products/{id}/listing/{}/delete" class="inline"><input type="hidden" name="csrf" value="{csrf}"><button class="btn btn-sm btn-danger" type="submit">Delete</button></form></td></tr>"#,
                escape(&p.name),
                p.days,
                p.id,
            )
        }).collect()
    };
    format!(
        r#"<div class="page narrow">
<div class="page-head"><div>
<h1>Plans — {}</h1>
<p>Define pricing plans for <strong>{}</strong>. The first plan in the list is what server owners see by default. Free plans (0 cents) offer a one-time trial per server.</p>
</div></div>
<section class="card">
<table class="table">
<thead><tr><th>Name</th><th>Price</th><th>Duration</th><th></th></tr></thead>
<tbody>{rows}</tbody>
</table>
</section>
<section class="card">
<h2>Add a plan</h2>
<form method="post" action="/products/{id}/listing">
<input type="hidden" name="csrf" value="{csrf}">
<div class="field"><label for="plan-name">Name</label><input id="plan-name" class="input" name="name" required maxlength="80" placeholder="e.g. Premium monthly"></div>
<div class="field"><label for="plan-price">Price (cents)</label><input id="plan-price" class="input" name="price_cents" type="number" min="0" required placeholder="e.g. 499 for $4.99 — 0 for free"></div>
<div class="field"><label for="plan-days">Duration (days)</label><input id="plan-days" class="input" name="days" type="number" min="1" max="3650" required placeholder="e.g. 30"></div>
<div class="field-row"><button class="btn btn-primary" type="submit">Add plan</button></div>
</form>
</section>
</div>"#,
        escape(name),
        escape(name),
    )
}

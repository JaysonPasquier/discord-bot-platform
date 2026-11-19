// Signalements de bugs et feedbacks sur un bot (visibles publiquement).
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use platform_core::ProductId;
use platform_store::{NewReport, Report};
use serde::Deserialize;

const MAX_REPORTS_PER_HOUR: u32 = 5;
const REPORT_WINDOW_SECS: i64 = 3600;

use crate::html::escape;
use crate::render::{message_back, not_found, redirect, require, require_csrf, server_error};
use crate::state::WebState;
use crate::views::ago;

#[derive(Deserialize)]
pub(crate) struct ReportForm {
    csrf: String,
    kind: String,
    title: String,
    body: String,
}

#[derive(Deserialize)]
pub(crate) struct RespondForm {
    csrf: String,
    status: String,
    dev_response: String,
}

pub(crate) async fn submit_report(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<ReportForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let back = format!("/bots/{id}#reports");
    if !["bug", "feedback"].contains(&form.kind.as_str()) {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid kind",
            "Kind must be 'bug' or 'feedback'.",
            &back,
        );
    }
    let title = form.title.trim().to_string();
    let body = form.body.trim().to_string();
    if title.is_empty() || title.len() > 120 {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid title",
            "Title must be 1–120 characters.",
            &back,
        );
    }
    if body.is_empty() || body.len() > 2000 {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid body",
            "Body must be 1–2000 characters.",
            &back,
        );
    }
    let now = (state.clock)();
    // Un seul verrou pour l'existence du produit, la limite de débit et l'insertion :
    // ferme la fenêtre de course entre la vérification de la limite et l'écriture.
    let Ok(store) = state.store.lock() else {
        return server_error(&state, Some(&auth));
    };
    let Ok(Some(_)) = store.product(ProductId(id)) else {
        return not_found(&state, &auth);
    };
    // Rate limit : max 5 reports par utilisateur par produit par heure. En panne, laisse
    // passer plutôt que de bloquer un report légitime (limite souple, pas une garantie dure).
    let recent = store
        .count_recent_reports_by(
            ProductId(id),
            auth.user_id,
            now.saturating_sub(REPORT_WINDOW_SECS),
        )
        .ok();
    if recent.is_some_and(|n| n >= MAX_REPORTS_PER_HOUR) {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Rate limit",
            "You have submitted too many reports. Try again in an hour.",
            &back,
        );
    }
    let saved = store.create_report(
        NewReport {
            product_id: ProductId(id),
            reporter_discord_id: Some(auth.user_id),
            reporter_name: &auth.name,
            kind: &form.kind,
            title: &title,
            body: &body,
        },
        now,
    );
    drop(store);
    if saved.is_ok() {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

pub(crate) async fn respond_to_report(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((id, report_id)): Path<(u64, i64)>,
    Form(form): Form<RespondForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let back = format!("/products/{id}#reports");
    let owned = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.product(ProductId(id)).ok().flatten())
        .filter(|p| p.owner == auth.user_id);
    if owned.is_none() {
        return not_found(&state, &auth);
    }
    if !["open", "in_progress", "fixed", "closed"].contains(&form.status.as_str()) {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid status",
            "Invalid report status.",
            &back,
        );
    }
    let now = (state.clock)();
    let saved = state.store.lock().map(|s| {
        s.update_report(
            report_id,
            ProductId(id),
            &form.status,
            form.dev_response.trim(),
            now,
        )
    });
    if matches!(saved, Ok(Ok(true))) {
        redirect(&back)
    } else {
        server_error(&state, Some(&auth))
    }
}

pub fn status_badge(status: &str) -> &'static str {
    match status {
        "open" => r#"<span class="chip chip-warn">Open</span>"#,
        "in_progress" => r#"<span class="chip chip-info">In progress</span>"#,
        "fixed" => r#"<span class="chip chip-ok">Fixed</span>"#,
        "closed" => r#"<span class="chip">Closed</span>"#,
        _ => "",
    }
}

pub fn kind_badge(kind: &str) -> &'static str {
    match kind {
        "bug" => r#"<span class="chip chip-danger">Bug</span>"#,
        "feedback" => r#"<span class="chip">Feedback</span>"#,
        _ => "",
    }
}

/// Section reports pour la page publique d'un bot (lecture seule + formulaire de soumission).
pub fn public_reports_section(reports: &[Report], product_id: u64, csrf: &str, now: i64) -> String {
    let form = format!(
        r#"<div class="section-head" id="reports"><h2>Reports &amp; feedback</h2></div>
<form class="card report-form" method="post" action="/bots/{product_id}/reports">
<input type="hidden" name="csrf" value="{csrf}">
<div class="field">
  <label for="rp-kind">Type</label>
  <select class="input" id="rp-kind" name="kind">
    <option value="bug">Bug report</option>
    <option value="feedback">Feedback</option>
  </select>
</div>
<div class="field">
  <label for="rp-title">Title</label>
  <input class="input" id="rp-title" name="title" maxlength="120" required placeholder="Short description">
</div>
<div class="field">
  <label for="rp-body">Details</label>
  <textarea class="input" id="rp-body" name="body" rows="4" maxlength="2000" required placeholder="Steps to reproduce, screenshots…"></textarea>
</div>
<div class="s-foot"><span></span><button class="btn btn-sm btn-primary" type="submit">Submit</button></div>
</form>"#
    );
    if reports.is_empty() {
        return format!("{form}<p class=\"faint\">No reports yet.</p>");
    }
    let items: String = reports
        .iter()
        .map(|r| {
            let response = if r.dev_response.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="report-response"><strong>Developer response:</strong> {}</div>"#,
                    escape(&r.dev_response)
                )
            };
            format!(
                r#"<div class="report-item">{kind} {status}<h3>{title}</h3><p class="faint small">{reporter} · {ago}</p><p>{body}</p>{response}</div>"#,
                kind = kind_badge(&r.kind),
                status = status_badge(&r.status),
                title = escape(&r.title),
                reporter = if r.reporter_name.is_empty() { "Anonymous".to_string() } else { escape(&r.reporter_name) },
                ago = ago(now, r.created_at),
                body = escape(&r.body),
            )
        })
        .collect();
    format!("{form}<div class=\"report-list\">{items}</div>")
}

/// Section reports pour le dashboard du développeur (avec formulaire de réponse).
pub fn dev_reports_section(reports: &[Report], product_id: u64, csrf: &str, now: i64) -> String {
    if reports.is_empty() {
        return r#"<div class="section-head" id="reports"><h2>Reports</h2></div><p class="faint">No reports yet.</p>"#
            .to_string();
    }
    let items: String = reports
        .iter()
        .map(|r| {
            let response_field = format!(
                r#"<div class="field">
  <label for="resp-{id}">Your response</label>
  <input class="input" id="resp-{id}" name="dev_response" value="{current}" maxlength="500" placeholder="Optional response visible to everyone">
</div>"#,
                id = r.id,
                current = escape(&r.dev_response)
            );
            format!(
                r#"<div class="report-item">{kind} {status}<h3>{title}</h3>
<p class="faint small">{reporter} · {ago}</p><p>{body}</p>
<form class="report-respond" method="post" action="/products/{product_id}/reports/{report_id}/respond">
<input type="hidden" name="csrf" value="{csrf}">
{response_field}
<div class="field"><label for="st-{id}">Status</label>
<select class="input" id="st-{id}" name="status">
  <option value="open"{open}>Open</option>
  <option value="in_progress"{in_prog}>In progress</option>
  <option value="fixed"{fixed}>Fixed</option>
  <option value="closed"{closed}>Closed</option>
</select></div>
<div class="s-foot"><span></span><button class="btn btn-sm" type="submit">Save</button></div>
</form></div>"#,
                kind = kind_badge(&r.kind),
                status = status_badge(&r.status),
                title = escape(&r.title),
                reporter = if r.reporter_name.is_empty() { "Anonymous".to_string() } else { escape(&r.reporter_name) },
                ago = ago(now, r.created_at),
                body = escape(&r.body),
                report_id = r.id,
                id = r.id,
                csrf = escape(csrf),
                open = if r.status == "open" { " selected" } else { "" },
                in_prog = if r.status == "in_progress" { " selected" } else { "" },
                fixed = if r.status == "fixed" { " selected" } else { "" },
                closed = if r.status == "closed" { " selected" } else { "" },
            )
        })
        .collect();
    format!(
        r#"<div class="section-head" id="reports"><h2>Reports</h2></div><div class="report-list">{items}</div>"#
    )
}

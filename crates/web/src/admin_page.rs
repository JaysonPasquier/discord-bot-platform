// Page HTML d'administration réservée à PLATFORM_ADMIN_USER_ID.
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_store::Deployment;
use serde::Deserialize;
use std::collections::HashMap;

use crate::html::{escape, AccountTab, Nav};
use crate::render::{not_found, page_in, redirect, require, require_csrf, server_error};
use crate::state::WebState;
use crate::views::{ago, status_badge};

pub(crate) async fn show(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !is_admin(&state, &auth) {
        return not_found(&state, &auth);
    }
    let admin_id = state.config.admin_user_id.as_deref().unwrap_or_default();

    let now = (state.clock)();
    let store = match state.store.lock() {
        Ok(s) => s,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let users = match store.all_users() {
        Ok(u) => u,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let bots = match store.all_products() {
        Ok(b) => b,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let entitlements = match store.all_entitlements() {
        Ok(e) => e,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let deployments = match store.recent_deployments_all(100) {
        Ok(d) => d,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let revenue_cents = match store.total_revenue_all() {
        Ok(r) => r,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let mrr_cents = match store.revenue_all_since(now - 30 * 86_400) {
        Ok(r) => r,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    drop(store);

    let active_subs = entitlements
        .iter()
        .filter(|e| !e.revoked && !e.canceled && e.paid_until > now)
        .count();

    let user_map: HashMap<u64, &str> = users.iter().map(|(id, n)| (*id, n.as_str())).collect();
    let product_map: HashMap<u64, &str> = bots.iter().map(|b| (b.id.0, b.name.as_str())).collect();

    let revenue_fmt = format!("${}.{:02}", revenue_cents / 100, revenue_cents % 100);
    let mrr_fmt = format!("${}.{:02}", mrr_cents / 100, mrr_cents % 100);
    let csrf = escape(&auth.csrf);

    let bots_rows: String = bots
        .iter()
        .map(|b| {
            let owner = user_map.get(&b.owner).copied().unwrap_or("?");
            let (status_chip, action_btn) = if b.suspended {
                (
                    r#"<span class="chip chip-err">Suspended</span>"#.to_string(),
                    format!(
                        r#"<form class="inline" method="post" action="/admin/bots/{id}/unsuspend"><input type="hidden" name="csrf" value="{csrf}"><button class="btn btn-xs" type="submit">Unsuspend</button></form>"#,
                        id = b.id.0
                    ),
                )
            } else {
                (
                    String::new(),
                    format!(
                        r#"<form class="inline" method="post" action="/admin/bots/{id}/suspend"><input type="hidden" name="csrf" value="{csrf}"><button class="btn btn-xs btn-danger" type="submit">Suspend</button></form>"#,
                        id = b.id.0
                    ),
                )
            };
            format!(
                r#"<tr><td class="strong">{name}{status_chip}</td><td class="faint">{owner}</td><td class="mono xs faint">{id}</td><td>{action_btn}</td></tr>"#,
                name = escape(&b.name),
                owner = escape(owner),
                id = b.id.0,
            )
        })
        .collect();

    let users_rows: String = users
        .iter()
        .map(|(id, name)| {
            format!(
                r#"<tr><td class="strong">{name}</td><td class="mono xs faint">{id}</td></tr>"#,
                name = escape(name),
            )
        })
        .collect();

    let subs_rows: String = entitlements
        .iter()
        .map(|e| {
            let (label, cls) = if e.revoked {
                ("Revoked", "chip-err")
            } else if e.canceled {
                ("Cancelled", "chip")
            } else if e.paid_until <= now {
                ("Expired", "chip-err")
            } else {
                ("Active", "chip-ok")
            };
            let product = product_map.get(&e.product.0).copied().unwrap_or("?");
            format!(
                r#"<tr><td class="mono xs faint">{guild}</td><td>{product}</td><td><span class="chip {cls}">{label}</span></td></tr>"#,
                guild = e.guild.0,
                product = escape(product),
            )
        })
        .collect();

    let deps_rows: String = deployments
        .iter()
        .take(30)
        .map(|d| {
            let product = product_map.get(&d.product.0).copied().unwrap_or("?");
            format!(
                r#"<tr><td>{product}</td><td class="faint small">{tier}</td><td>{status}</td><td class="faint small">{ago}</td></tr>"#,
                product = escape(product),
                tier = escape(&d.tier),
                status = status_badge(d.status),
                ago = ago(now, d.created_at),
            )
        })
        .collect();

    let empty = r#"<tr class="admin-table-empty"><td colspan="99">Empty</td></tr>"#;
    let bots_rows = if bots_rows.is_empty() {
        empty.to_string()
    } else {
        bots_rows
    };
    let users_rows = if users_rows.is_empty() {
        empty.to_string()
    } else {
        users_rows
    };
    let subs_rows = if subs_rows.is_empty() {
        empty.to_string()
    } else {
        subs_rows
    };
    let deps_rows = if deps_rows.is_empty() {
        empty.to_string()
    } else {
        deps_rows
    };

    let chart_svg = deploy_chart(&deployments, now);

    let body = format!(
        r#"<div class="page-wide">
<div class="admin-header"><h1>Admin</h1><span class="chip chip-warn">Restricted</span></div>
<div class="admin-kpis">
  <div class="admin-kpi"><span>Users</span><b>{users_n}</b></div>
  <div class="admin-kpi"><span>Bots</span><b>{bots_n}</b></div>
  <div class="admin-kpi"><span>Active subscriptions</span><b>{active_subs}</b></div>
  <div class="admin-kpi"><span>Total revenue</span><b class="text-ok">{revenue_fmt}</b></div>
  <div class="admin-kpi"><span>MRR (30 days)</span><b class="text-ok">{mrr_fmt}</b></div>
</div>
<div class="admin-layout">
  <div class="admin-main">
    <div class="card">
      <div class="card-head"><h3>Deployment activity</h3><span class="faint small">Last 7 days</span></div>
      <div class="admin-chart-body">{chart_svg}</div>
    </div>
    <div class="card">
      <div class="card-head"><h3>Bots</h3><span class="chip">{bots_n}</span></div>
      <table class="admin-table"><thead><tr><th>Name</th><th>Owner</th><th>ID</th><th><span class="sr-only">Actions</span></th></tr></thead><tbody>{bots_rows}</tbody></table>
    </div>
    <div class="card">
      <div class="card-head"><h3>Users</h3><span class="chip">{users_n}</span></div>
      <table class="admin-table"><thead><tr><th>Name</th><th>Discord ID</th></tr></thead><tbody>{users_rows}</tbody></table>
    </div>
    <div class="card">
      <div class="card-head"><h3>Subscriptions</h3><span class="chip">{subs_n}</span></div>
      <table class="admin-table"><thead><tr><th>Server ID</th><th>Bot</th><th>Status</th></tr></thead><tbody>{subs_rows}</tbody></table>
    </div>
    <div class="card">
      <div class="card-head"><h3>Recent deployments</h3></div>
      <table class="admin-table"><thead><tr><th>Bot</th><th>Tier</th><th>Status</th><th>When</th></tr></thead><tbody>{deps_rows}</tbody></table>
    </div>
  </div>
  <aside class="admin-aside">
    <div class="card card-pad">
      <div class="strong">Quick actions</div>
      <div class="admin-actions">
        <a class="btn btn-sm btn-block" href="/api/admin/overview">Raw JSON overview</a>
      </div>
    </div>
    <div class="card card-pad">
      <div class="strong">Platform info</div>
      <dl class="admin-info">
        <div class="admin-info-row"><dt>Admin user ID</dt><dd class="mono xs faint">{admin_id}</dd></div>
        <div class="admin-info-row"><dt>Total bots</dt><dd>{bots_n}</dd></div>
        <div class="admin-info-row"><dt>Total users</dt><dd>{users_n}</dd></div>
        <div class="admin-info-row"><dt>Active subscriptions</dt><dd class="text-ok">{active_subs}</dd></div>
        <div class="admin-info-row"><dt>Total revenue</dt><dd class="text-ok">{revenue_fmt}</dd></div>
      </dl>
    </div>
  </aside>
</div>
</div>"#,
        users_n = users.len(),
        bots_n = bots.len(),
        subs_n = entitlements.len(),
        active_subs = active_subs,
        revenue_fmt = revenue_fmt,
        mrr_fmt = mrr_fmt,
        admin_id = escape(admin_id),
        chart_svg = chart_svg,
        bots_rows = bots_rows,
        users_rows = users_rows,
        subs_rows = subs_rows,
        deps_rows = deps_rows,
    );

    page_in(
        &auth,
        &state,
        "Admin",
        &body,
        &Nav::Account(AccountTab::Bots),
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct AdminCsrf {
    csrf: String,
}

fn is_admin(state: &WebState, auth: &crate::session::Auth) -> bool {
    state
        .config
        .admin_user_id
        .as_deref()
        .is_some_and(|id| auth.user_id.to_string() == id)
}

pub(crate) async fn suspend(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<AdminCsrf>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !is_admin(&state, &auth) {
        return not_found(&state, &auth);
    }
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let _ = state.store.lock().map(|s| s.suspend_product(ProductId(id)));
    redirect("/admin")
}

pub(crate) async fn unsuspend(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<AdminCsrf>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !is_admin(&state, &auth) {
        return not_found(&state, &auth);
    }
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let _ = state
        .store
        .lock()
        .map(|s| s.unsuspend_product(ProductId(id)));
    redirect("/admin")
}

// Graphe SVG des déploiements sur les 7 derniers jours (attributs de présentation SVG, pas de style inline).
fn deploy_chart(deployments: &[Deployment], now: i64) -> String {
    let mut counts = [0u32; 7];
    for d in deployments {
        let days_ago = (now - d.created_at) / 86_400;
        if (0..7).contains(&days_ago) {
            counts[(6 - days_ago) as usize] += 1;
        }
    }
    let max = counts.iter().copied().max().unwrap_or(1).max(1);

    let bar_w: u32 = 32;
    let gap: u32 = 10;
    let bar_max_h: u32 = 60;
    let label_h: u32 = 20;
    let total_h = bar_max_h + 8 + label_h;
    let total_w = 7 * (bar_w + gap) - gap;

    let bars: String = counts
        .iter()
        .enumerate()
        .map(|(i, &count)| {
            let x = i as u32 * (bar_w + gap);
            if count == 0 {
                let y = bar_max_h.saturating_sub(2);
                format!(r#"<rect x="{x}" y="{y}" width="{bar_w}" height="2" rx="1" fill="var(--line-2)"/>"#)
            } else {
                let bar_h = (count * bar_max_h / max).max(4);
                let y = bar_max_h.saturating_sub(bar_h);
                format!(r#"<rect x="{x}" y="{y}" width="{bar_w}" height="{bar_h}" rx="4" fill="var(--blue)"/>"#)
            }
        })
        .collect();

    // (days_since_epoch + 4) % 7 → 0=Thu,1=Fri,2=Sat,3=Sun,4=Mon,5=Tue,6=Wed
    // On mappe : [0=Sun,1=Mon,2=Tue,3=Wed,4=Thu,5=Fri,6=Sat]
    // (d + 4) % 7 avec epoch=Thu → index 4 → "Thu" ✓
    let day_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let labels: String = (0usize..7)
        .map(|i| {
            let day_ts = now - (6 - i as i64) * 86_400;
            let days_since_epoch = day_ts.div_euclid(86_400);
            let dow = ((days_since_epoch + 4).rem_euclid(7)) as usize;
            let name = day_names[dow];
            let cx = i as u32 * (bar_w + gap) + bar_w / 2;
            let ty = bar_max_h + 8 + 11;
            format!(r#"<text x="{cx}" y="{ty}" text-anchor="middle" font-size="11" font-family="system-ui,sans-serif" fill="var(--fg-3)">{name}</text>"#)
        })
        .collect();

    format!(
        r#"<svg viewBox="0 0 {total_w} {total_h}" width="{total_w}" height="{total_h}" role="img" aria-label="Deployment activity">{bars}{labels}</svg>"#,
    )
}

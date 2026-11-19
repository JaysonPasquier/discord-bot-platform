use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_runner::{container_name, tier_name, ContainerState, Tier};
use platform_store::{BotCredential, DeployStatus, Deployment, Source};

use crate::html::{escape, Nav, ProjectTab};
use crate::icons::{self, bot_icon, dc_profile};
use crate::products::owned_product;
use crate::render::{page_in, require, server_error};
use crate::session::Auth;
use crate::state::WebState;
use crate::views::{ago, branch_pill, dep_row, short, status_badge, status_label, tier_title};

const HISTORY_ROWS: u32 = 5;
// Assez pour le compteur de l'onglet « Deployments » et le total des 7 derniers jours.
pub(crate) const COUNT_ROWS: u32 = 500;

pub(crate) fn nav<'a>(state: &WebState, id: u64, name: &'a str, tab: ProjectTab) -> Nav<'a> {
    let deployments = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.deployments_of(ProductId(id), COUNT_ROWS).ok())
        .map_or(0, |d| d.len());
    Nav::Project {
        id,
        name,
        tab,
        deployments,
    }
}

struct Version {
    tier: Tier,
    source: Option<Source>,
    credential: Option<BotCredential>,
    deployments: Vec<Deployment>,
    container: Option<Option<ContainerState>>,
    logs: String,
}

fn container_text(state: &Option<Option<ContainerState>>) -> String {
    match state {
        None => r#"<span class="faint">Docker is not reachable</span>"#.to_string(),
        Some(None) => r#"<span class="faint">No container</span>"#.to_string(),
        Some(Some(ContainerState::Running)) => "Running".to_string(),
        Some(Some(ContainerState::Restarting)) => {
            r#"<span class="chip chip-err">Restarting</span> <span class="faint small">It keeps crashing. Check the logs.</span>"#
                .to_string()
        }
        Some(Some(ContainerState::Exited(code))) => format!("Stopped (exit code {code})"),
        Some(Some(ContainerState::Other(other))) => escape(other),
    }
}

fn current(v: &Version) -> Option<&Deployment> {
    v.deployments
        .iter()
        .find(|d| d.status == DeployStatus::Running)
}

fn in_progress(v: &Version) -> Option<&Deployment> {
    v.deployments.iter().find(|d| d.status.is_active())
}

fn kv(dt: &str, class: &str, dd: &str) -> String {
    let class = if class.is_empty() {
        String::new()
    } else {
        format!(r#" class="{class}""#)
    };
    format!("<div><dt>{dt}</dt><dd{class}>{dd}</dd></div>")
}

fn source_dd(id: u64, v: &Version) -> String {
    match &v.source {
        None => format!(
            r#"<a class="link" href="/products/{id}/source/{}">Connect a repository</a>"#,
            tier_name(v.tier)
        ),
        Some(s) => format!(
            r#"{}<a class="truncate" href="/products/{id}/source">{}</a>{}"#,
            icons::GITHUB,
            escape(&s.repo),
            branch_pill(&s.branch)
        ),
    }
}

fn discord_dd(id: u64, v: &Version) -> String {
    match &v.credential {
        None => format!(r#"<a class="link" href="/products/{id}/bot">Add the bot token</a>"#),
        Some(c) => format!(
            r#"{} <span class="faint mono small">{}</span>"#,
            escape(&c.bot_name),
            escape(&c.client_id)
        ),
    }
}

fn prod_block(id: u64, name: &str, v: &Version, csrf: &str, now: i64) -> String {
    let tier = tier_name(v.tier);
    let title = tier_title(tier);
    let app = v.credential.as_ref().map_or_else(
        || {
            if v.tier == Tier::Premium {
                format!("{name} Premium")
            } else {
                name.to_string()
            }
        },
        |c| c.bot_name.clone(),
    );
    let bio = if v.tier == Tier::Premium {
        "Premium version."
    } else {
        "Free version."
    };
    let shot = format!(
        r#"<div class="prod-shot">{}</div>"#,
        dc_profile(id, &app, bio)
    );
    let Some(source) = &v.source else {
        let has_container = matches!(v.container, Some(Some(_)));
        let details = if v.credential.is_some() || has_container {
            format!(
                r#"<dl class="kv">{}{}</dl>"#,
                kv("Container", "", &container_text(&v.container)),
                kv("Discord application", "", &discord_dd(id, v))
            )
        } else {
            String::new()
        };
        let logs = if v.logs.trim().is_empty() {
            String::new()
        } else {
            format!(
                r#"<details class="logs" data-live="logs-{tier}"><summary>{}<span>Container logs</span></summary><div class="logs-body" tabindex="0"><pre>{}</pre></div></details>"#,
                icons::CHEV,
                escape(&v.logs)
            )
        };
        let example = if v.credential.is_some() {
            format!(
                r#"<form class="inline" method="post" action="/products/{id}/deploy"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="tier" value="{tier}"><button class="btn" type="submit">{} Run the example bot</button></form>"#,
                icons::ROCKET
            )
        } else {
            String::new()
        };
        return format!(
            r#"<section class="prod" id="{tier}" aria-label="{title} version">{shot}<div class="prod-info"><div class="prod-title"><h2>{title}</h2><span class="faint">Not deployed</span></div>
<div class="empty"><h3>No repository</h3><p>Connect a repository to deploy the {tier} version.</p><div class="row-wrap"><a class="btn btn-primary" href="/products/{id}/source/{tier}">{github} Connect repository</a>{example}</div></div>{details}{logs}</div></section>"#,
            github = icons::GITHUB
        );
    };
    let cur = current(v);
    let busy = in_progress(v);
    let state = cur.map_or_else(
        || {
            busy.map_or_else(
                || r#"<span class="faint">Not deployed</span>"#.to_string(),
                |d| status_badge(d.status),
            )
        },
        |_| status_badge(DeployStatus::Running),
    );
    let building = busy.map_or_else(String::new, |d| {
        format!(
            r#"<a class="building-bar" href="/products/{id}/deployments/{}"><span>{} <span class="sha">{}</span></span><span class="bar"><i></i></span><span class="link">View logs</span></a>"#,
            d.id,
            status_label(d.status),
            escape(short(&d.commit)),
        )
    });
    let deployment = cur.map_or_else(
        || "None yet".to_string(),
        |d| {
            format!(
                r#"<a class="link mono" href="/products/{id}/deployments/{}">{}</a>"#,
                d.id,
                escape(short(&d.commit))
            )
        },
    );
    let created = cur.map_or_else(
        || "Not yet".to_string(),
        |d| format!("{} by {}", ago(now, d.created_at), escape(&d.trigger)),
    );
    let automatic = if source.auto_deploy {
        format!(
            r#"On for pushes to <span class="mono">{}</span>"#,
            escape(&source.branch)
        )
    } else {
        "Off".to_string()
    };
    let rows = [
        kv("Deployment", "", &deployment),
        kv("Created", "", &created),
        kv("Source", "row", &source_dd(id, v)),
        kv("Automatic deploys", "", &automatic),
        kv("Container", "", &container_text(&v.container)),
        kv("Discord application", "", &discord_dd(id, v)),
    ]
    .concat();
    let hidden = format!(
        r#"<input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="tier" value="{tier}">"#
    );
    let deploy = if v.credential.is_some() {
        format!(
            r#"<form class="inline" method="post" action="/products/{id}/deployments">{hidden}<input type="hidden" name="branch" value="{}"><button class="btn btn-primary btn-sm" type="submit">{} Deploy latest</button></form><a class="btn btn-sm" href="/products/{id}/deploy/{tier}">{} Choose a branch</a>"#,
            escape(&source.branch),
            icons::ROCKET,
            icons::BRANCH
        )
    } else {
        format!(
            r#"<span class="faint small">Add the bot token to deploy.</span><a class="btn btn-sm" href="/products/{id}/bot">Add the bot token</a>"#
        )
    };
    let logs = if v.logs.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<details class="logs" data-live="logs-{tier}"><summary>{}<span>Container logs</span></summary><div class="logs-body" tabindex="0"><pre>{}</pre></div></details>"#,
            icons::CHEV,
            escape(&v.logs)
        )
    };
    format!(
        r#"<section class="prod" id="{tier}" aria-label="{title} version">{shot}
<div class="prod-info"><div class="prod-title"><h2>{title}</h2>{state}</div>{building}
<dl class="kv">{rows}</dl>
<div class="prod-actions">{deploy}<form class="inline" method="post" action="/products/{id}/stop">{hidden}<button class="btn btn-sm btn-ghost" type="submit">{stop} Stop</button></form></div>{logs}</div></section>"#,
        stop = icons::STOP
    )
}

// Ce qui reste à faire avant que le bot soit en ligne, pour qu'un nouveau créateur sache toujours la suite.
fn setup(id: u64, versions: &[Version]) -> String {
    let has = |tier: Tier, f: fn(&Version) -> bool| versions.iter().any(|v| v.tier == tier && f(v));
    let steps = [
        (
            "Connect a repository to the free version",
            has(Tier::Free, |v| v.source.is_some()),
            format!("/products/{id}/source/free"),
        ),
        (
            "Connect a repository to the premium version",
            has(Tier::Premium, |v| v.source.is_some()),
            format!("/products/{id}/source/premium"),
        ),
        (
            "Add the free bot token",
            has(Tier::Free, |v| v.credential.is_some()),
            format!("/products/{id}/bot"),
        ),
        (
            "Add the premium bot token",
            has(Tier::Premium, |v| v.credential.is_some()),
            format!("/products/{id}/bot"),
        ),
        (
            "Deploy a version",
            versions.iter().any(|v| current(v).is_some()),
            format!("/products/{id}/deployments"),
        ),
    ];
    if steps.iter().all(|s| s.1) {
        return String::new();
    }
    let done = steps.iter().filter(|s| s.1).count();
    let items: String = steps
        .iter()
        .map(|(text, ok, href)| {
            let (dot, class, link) = if *ok {
                (format!(" done\">{}", icons::CHECK), "muted", "")
            } else {
                ("\">".to_string(), "strong", r#"<span class="link small">Do this</span>"#)
            };
            format!(
                r#"<a class="item hover" href="{href}"><span class="check-dot{dot}</span><span class="grow {class}">{text}</span>{link}</a>"#
            )
        })
        .collect();
    format!(
        r#"<div class="card section-gap"><div class="card-head"><h3 class="grow">Finish setting up</h3><span class="chip">{done} of {}</span></div><div class="list">{items}</div></div>"#,
        steps.len()
    )
}

fn deploy_menu(id: u64, versions: &[Version], csrf: &str) -> String {
    let ready: Vec<&Version> = versions
        .iter()
        .filter(|v| v.source.is_some() && v.credential.is_some())
        .collect();
    let mut items = String::new();
    for v in &ready {
        let (tier, branch) = (
            tier_name(v.tier),
            v.source.as_ref().map_or("main", |s| s.branch.as_str()),
        );
        items.push_str(&format!(
            r#"<form method="post" action="/products/{id}/deployments"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="tier" value="{tier}"><input type="hidden" name="branch" value="{b}"><button role="menuitem" type="submit">{title}, latest from {b}</button></form>"#,
            b = escape(branch),
            title = tier_title(tier)
        ));
    }
    match ready.first().map(|v| tier_name(v.tier)) {
        Some(tier) => items.push_str(&format!(
            r#"<hr><a role="menuitem" href="/products/{id}/deploy/{tier}">Choose a branch...</a>"#
        )),
        None => items.push_str(&format!(
            r#"<a role="menuitem" href="/products/{id}/source/free">Connect a repository first</a>"#
        )),
    }
    format!(
        r#"<details class="menu"><summary class="btn btn-primary">Deploy {}</summary><div class="menu-pop" role="menu">{items}</div></details>"#,
        icons::DOWN
    )
}

fn stats(premium: usize, all: &[Deployment], now: i64) -> String {
    let cell = |label: &str, value: &str, hint: &str| {
        format!(
            r#"<div class="stat-cell"><span class="faint small">{label}</span><b>{value}</b><span class="small muted">{hint}</span></div>"#
        )
    };
    let week_count = all
        .iter()
        .filter(|d| now - d.created_at < 7 * 86_400)
        .count();
    let (ready_n, tried_n) = all.iter().filter(|d| now - d.created_at < 7 * 86_400).fold(
        (0usize, 0usize),
        |(r, t), d| match d.status {
            DeployStatus::Ready => (r + 1, t + 1),
            DeployStatus::Failed => (r, t + 1),
            _ => (r, t),
        },
    );
    let success_rate = match (ready_n * 100).checked_div(tried_n) {
        Some(pct) => format!("{pct}%"),
        None => "—".to_string(),
    };
    let avg_secs: Option<i64> = {
        let finished: Vec<i64> = all
            .iter()
            .filter_map(|d| {
                let f = d.finished_at?;
                Some(f - d.created_at)
            })
            .filter(|&s| s > 0 && s < 3600)
            .collect();
        if finished.is_empty() {
            None
        } else {
            Some(finished.iter().sum::<i64>() / finished.len() as i64)
        }
    };
    let avg_build = avg_secs.map_or("—".to_string(), |s| {
        if s < 60 {
            format!("{s}s")
        } else {
            format!("{}m{}s", s / 60, s % 60)
        }
    });
    format!(
        r#"<div class="stats" aria-label="Project totals">{}{}{}{}</div>"#,
        cell("Servers", "—", "Not tracked yet"),
        cell(
            "Premium subscribers",
            &premium.to_string(),
            "Active or in grace"
        ),
        cell("Deploys (7d)", &week_count.to_string(), &success_rate),
        cell("Avg build time", &avg_build, "All history"),
    )
}

pub(crate) fn deps_html(
    rows: &[Deployment],
    csrf: &str,
    now: i64,
    user: u64,
    empty: &str,
) -> String {
    if rows.is_empty() {
        return format!(
            r#"<div class="empty"><h3>{empty}</h3><p>Connect a repository, then deploy.</p></div>"#
        );
    }
    rows.iter().map(|d| dep_row(d, csrf, now, user)).collect()
}

fn gather(state: &WebState, id: u64) -> Option<(Vec<Version>, Vec<Deployment>, usize)> {
    let store = state.store.lock().ok()?;
    let product = ProductId(id);
    let all = store.deployments_of(product, COUNT_ROWS).ok()?;
    let now = (state.clock)();
    let premium = store
        .entitlements_of(product)
        .ok()?
        .iter()
        .filter(|e| crate::billing::counts_as_active_subscriber(e, now, state.config.grace_secs))
        .count();
    let mut versions = Vec::new();
    for tier in [Tier::Free, Tier::Premium] {
        let name = tier_name(tier);
        versions.push(Version {
            tier,
            source: store.source(product, name).ok()?,
            credential: store.credential(product, name).ok()?,
            deployments: all.iter().filter(|d| d.tier == name).cloned().collect(),
            container: None,
            logs: String::new(),
        });
    }
    Some((versions, all, premium))
}

pub(crate) async fn show(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth: Auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let Some((mut versions, all, premium)) = gather(&state, id) else {
        return server_error(&state, Some(&auth));
    };
    let containers = state.containers.clone();
    let states = tokio::task::spawn_blocking(move || {
        [Tier::Free, Tier::Premium].map(|tier| {
            let name = container_name(id, tier);
            let state = containers.state(&name).ok();
            let logs = if matches!(state, Some(Some(_))) {
                containers.logs(&name, 30).unwrap_or_default()
            } else {
                String::new()
            };
            (state, logs)
        })
    })
    .await;
    let Ok(states) = states else {
        return server_error(&state, Some(&auth));
    };
    for (version, (container, logs)) in versions.iter_mut().zip(states) {
        version.container = container;
        version.logs = logs;
    }
    let now = (state.clock)();
    let csrf = escape(&auth.csrf);
    let busy = all.iter().any(|d| d.status.is_active());
    let poll = if busy { "2" } else { "6" };
    let prods: String = versions
        .iter()
        .map(|v| prod_block(id, &product.name, v, &csrf, now))
        .collect();
    let recent: Vec<Deployment> = all.iter().take(HISTORY_ROWS as usize).cloned().collect();
    let body = format!(
        r#"<div data-poll="{poll}"><div class="page">
<div class="page-head"><div><div class="row">{icon}<h1>{name}</h1></div><p>Two Discord bots from one project. Deploy each version, watch its status and open the listing members see.</p></div>
<div class="row-wrap"><a class="btn" href="/bots/{id}">{external} Listing</a>{menu}</div></div>
{stats}
{setup}
<div data-live="prod">{prods}</div>
<div class="section"><div class="section-head"><h2>Recent deployments</h2><a class="link small" href="/products/{id}/deployments">View all</a></div>
<div class="deps"><div data-live="history">{deps}</div></div></div>
</div></div>"#,
        icon = bot_icon(id, 48),
        name = escape(&product.name),
        external = icons::EXTERNAL,
        menu = deploy_menu(id, &versions, &csrf),
        stats = stats(premium, &all, now),
        setup = setup(id, &versions),
        deps = deps_html(&recent, &csrf, now, auth.user_id, "No deployments yet"),
    );
    let nav = nav(&state, id, &product.name, ProjectTab::Overview);
    page_in(&auth, &state, &product.name, &body, &nav).into_response()
}

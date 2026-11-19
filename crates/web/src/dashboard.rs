use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use platform_core::GuildId;
use platform_runner::tier_name;
use platform_store::{DeployStatus, Deployment};

use crate::discord::invite_url;
use crate::html::{escape, AccountTab, Nav};
use crate::icons::{self, bot_icon};
use crate::render::{page_in, require, server_error};
use crate::session::Auth;
use crate::state::WebState;
use crate::summary::{gather_cards, BotCard, VersionSummary};
use crate::views::{ago, short, status_badge};

// Combien de bots lus d'un coup : bornent les requêtes faites pour le tableau de bord.
const ACTIVITY_ROWS: usize = 8;

pub(crate) fn guild_name(auth: &Auth, guild: GuildId) -> String {
    let id = guild.0.to_string();
    auth.guilds
        .iter()
        .find(|g| g.id == id)
        .map_or_else(|| format!("server {id}"), |g| g.name.clone())
}

pub(crate) fn invite_button(label: &str, client_id: &str) -> String {
    format!(
        r#"<a class="btn btn-sm" href="{}">{}{}</a>"#,
        escape(&invite_url(client_id)),
        icons::DISCORD,
        escape(label)
    )
}

fn version_state(v: &VersionSummary) -> String {
    match &v.latest {
        Some(d) if d.status == DeployStatus::Running => status_badge(DeployStatus::Running),
        Some(d) => status_badge(d.status),
        None => r#"<span class="faint">Not deployed</span>"#.to_string(),
    }
}

fn project_card(c: &BotCard) -> String {
    let repo = if c.free.has_source || c.premium.has_source {
        format!(
            r#"<span class="repo">{}<span class="truncate">Connected</span></span>"#,
            icons::GITHUB
        )
    } else {
        r#"<span class="repo">No repository connected</span>"#.to_string()
    };
    let app_line = c
        .free
        .app_name
        .clone()
        .or_else(|| c.premium.app_name.clone())
        .map_or_else(|| "No Discord application yet".to_string(), |n| escape(&n));
    let commit = c.last_deploy.as_ref().map_or_else(
        || {
            r#"<span class="msg">Nothing deployed yet</span><span>Connect a repository to deploy.</span>"#
                .to_string()
        },
        |d| {
            format!(
                r#"<span class="msg truncate"><code>{}</code></span><span class="row">on <span class="branch">{}</span></span>"#,
                escape(short(&d.commit)),
                escape(&d.branch),
            )
        },
    );
    format!(
        r#"<a class="project" href="/products/{id}">
<div class="top">{icon}<div class="grow"><div class="name">{name}</div><div class="faint small">{app_line}</div></div></div>
{repo}
<div class="commit">{commit}</div>
<div class="vers"><div><span class="muted">Free</span>{free}</div><div><span class="muted">Premium</span>{premium}</div></div>
<div class="mini-stats"><span>{server_icon}Not tracked yet</span><span>{users_icon}{subs} premium</span></div></a>"#,
        id = c.product.id.0,
        icon = bot_icon(c.product.id.0, 40),
        name = escape(&c.product.name),
        free = version_state(&c.free),
        premium = version_state(&c.premium),
        server_icon = icons::SERVER,
        users_icon = icons::USERS,
        subs = c.premium_subscribers,
    )
}

// « Needs attention » : ordre de priorité, un seul motif par version (comme la maquette).
// Pas de contrôle de container qui redémarre : il faudrait interroger Docker pour chaque bot
// à chaque chargement du tableau de bord.
struct Attention {
    product: u64,
    name: String,
    title: String,
    text: String,
    href: String,
    kind: &'static str,
}

fn attention_items(cards: &[BotCard]) -> Vec<Attention> {
    let mut items = Vec::new();
    for c in cards {
        for v in [&c.free, &c.premium] {
            let tier = tier_name(v.tier);
            let (kind, title, text, href) = if matches!(&v.latest, Some(d) if d.status == DeployStatus::Failed)
            {
                (
                    "chip-err",
                    format!("{tier} deployment failed"),
                    "The previous version is still live.".to_string(),
                    format!(
                        "/products/{}/deployments/{}",
                        c.product.id.0,
                        v.latest.as_ref().map_or(0, |d| d.id)
                    ),
                )
            } else if !v.has_source {
                (
                    "chip-warn",
                    format!("{tier} has no repository"),
                    "Connect one to deploy this version.".to_string(),
                    format!("/products/{}/source/{tier}", c.product.id.0),
                )
            } else if !v.has_credential {
                (
                    "chip-warn",
                    format!("{tier} has no bot token"),
                    "Add the token from the Discord Developer Portal.".to_string(),
                    format!("/products/{}#{tier}", c.product.id.0),
                )
            } else {
                continue;
            };
            items.push(Attention {
                product: c.product.id.0,
                name: c.product.name.clone(),
                title,
                text,
                href,
                kind,
            });
        }
    }
    items
}

fn activity_html(cards: &[BotCard], now: i64) -> String {
    let mut all: Vec<(&Deployment, u64, &str)> = Vec::new();
    for c in cards {
        for v in [&c.free, &c.premium] {
            if let Some(d) = &v.latest {
                all.push((d, c.product.id.0, c.product.name.as_str()));
            }
        }
    }
    all.sort_by_key(|(d, ..)| std::cmp::Reverse(d.created_at));
    if all.is_empty() {
        return r#"<p class="empty"><span></span><span>No deployment yet.</span></p>"#.to_string();
    }
    let rows: String = all
        .into_iter()
        .take(ACTIVITY_ROWS)
        .map(|(d, pid, name)| {
            format!(
                r#"<li>{icon}<div class="grow"><div class="row"><a class="strong" href="/products/{pid}/deployments/{did}">{name}</a><span class="faint">{tier}</span><span class="sha">{commit}</span></div>
<div class="row small">{state}<span class="faint">{ago}</span></div></div></li>"#,
                icon = bot_icon(pid, 24),
                did = d.id,
                name = escape(name),
                tier = escape(&d.tier),
                commit = escape(short(&d.commit)),
                state = status_badge(d.status),
                ago = ago(now, d.created_at),
            )
        })
        .collect();
    format!(r#"<ul class="feed">{rows}</ul>"#)
}

fn stats_html(cards: &[BotCard], now: i64) -> String {
    const WEEK_SECS: i64 = 7 * 86_400;
    let week = cards
        .iter()
        .flat_map(|c| [&c.free, &c.premium])
        .filter(|v| matches!(&v.latest, Some(d) if now - d.created_at < WEEK_SECS))
        .count();
    let premium: usize = cards.iter().map(|c| c.premium_subscribers).sum();
    let cell = |label: &str, value: &str, hint: &str| {
        format!(
            r#"<div class="stat-cell"><span class="faint small">{label}</span><b>{value}</b><span class="small muted">{hint}</span></div>"#
        )
    };
    format!(
        r#"<div class="stats" aria-label="Totals across your bots">{}{}{}{}</div>
<p class="hint stats-note"><span class="chip chip-outline">Partly tracked</span> Servers and total revenue are not aggregated across bots yet, the other two are real, read from the database.</p>"#,
        cell("Servers", "—", "Not tracked yet"),
        cell(
            "Premium subscribers",
            &premium.to_string(),
            "Active or in grace"
        ),
        cell("Monthly revenue", "—", "Not tracked yet"),
        cell("Deployments", &week.to_string(), "In the last 7 days"),
    )
}

pub(crate) async fn dashboard(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let now = (state.clock)();
    let Ok(store) = state.store.lock() else {
        return server_error(&state, Some(&auth));
    };
    let Ok(products) = store.products_of(auth.user_id) else {
        return server_error(&state, Some(&auth));
    };
    let Ok(cards) = gather_cards(&store, products, now, state.config.grace_secs) else {
        return server_error(&state, Some(&auth));
    };
    drop(store);

    let attention = attention_items(&cards);
    let needs_attention = if attention.is_empty() {
        String::new()
    } else {
        let items: String = attention
            .iter()
            .map(|a| {
                format!(
                    r#"<a class="item hover" href="{href}">{icon}<div class="grow"><div class="row-wrap"><span class="strong">{name}</span><span class="chip {kind}">{title}</span></div><div class="small muted">{text}</div></div><span class="faint">{chev}</span></a>"#,
                    href = a.href,
                    icon = bot_icon(a.product, 32),
                    name = escape(&a.name),
                    kind = a.kind,
                    title = escape(&a.title),
                    text = escape(&a.text),
                    chev = icons::CHEV,
                )
            })
            .collect();
        format!(
            r#"<div class="section"><div class="section-head"><h2>Needs attention</h2><span class="chip chip-err">{}</span></div><div class="card list">{items}</div></div>"#,
            attention.len()
        )
    };

    let projects = if cards.is_empty() {
        r#"<div class="card empty"><h3>No bot yet</h3><p>Create one, then connect its repository and its Discord applications.</p><a class="btn btn-primary" href="/new">New bot</a></div>"#
            .to_string()
    } else {
        format!(
            r#"<div id="proj-grid" class="projects">{}</div>"#,
            cards.iter().map(project_card).collect::<String>()
        )
    };
    let body = format!(
        r#"<div class="page">
<div class="page-head"><div><h1>Your bots</h1><p>Every bot you host, with its two versions. Open one to deploy, read logs or change its source.</p></div>
<a class="btn btn-primary" href="/new">{plus} New bot</a></div>
{stats}
{needs_attention}
<div class="section"><div class="toolbar"><label class="input-icon" for="proj-q"><span class="sr-only">Search your bots</span>{search}<input class="input" id="proj-q" type="search" placeholder="Search your bots" autocomplete="off"></label></div>
<div class="split">
<div>{projects}</div>
<aside><div class="section-head"><h2>Recent activity</h2></div>{activity}
<div class="card card-pad section"><div class="strong">Shortcuts</div><div class="shortcuts"><a class="link" href="/new">Create a bot</a><a class="link" href="/github">Choose GitHub repositories</a><a class="link" href="/docs/bot-toml">bot.toml reference</a><a class="link" href="/sdk">SDK reference</a></div></div></aside>
</div></div>
</div>"#,
        plus = icons::PLUS,
        search = icons::SEARCH,
        stats = stats_html(&cards, now),
        activity = activity_html(&cards, now),
    );
    page_in(
        &auth,
        &state,
        "Your bots",
        &body,
        &Nav::Account(AccountTab::Bots),
    )
    .into_response()
}

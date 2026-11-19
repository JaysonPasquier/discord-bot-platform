// Page publique d'un développeur : ses bots disponibles sur le marché.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use crate::html::escape;
use crate::icons::{self, bot_icon};
use crate::render::{not_found, page, require, server_error};
use crate::state::WebState;

pub(crate) async fn show(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(dev_id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let store = match state.store.lock() {
        Ok(s) => s,
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let profile = match store.user_profile(dev_id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            drop(store);
            return not_found(&state, &auth);
        }
        Err(_) => {
            drop(store);
            return server_error(&state, Some(&auth));
        }
    };
    let products = match store.products_of(dev_id) {
        Ok(p) => p,
        Err(_) => {
            drop(store);
            return server_error(&state, Some(&auth));
        }
    };
    let server_count = store.server_count_of(dev_id).unwrap_or(0);
    let listed: Vec<_> = products
        .iter()
        .filter(|p| !p.ceded && !p.suspended)
        .filter(|p| {
            store
                .credential(p.id, "free")
                .ok()
                .flatten()
                .or_else(|| store.credential(p.id, "premium").ok().flatten())
                .is_some()
        })
        .collect();
    drop(store);

    let bot_count = listed.len();
    let cards: String = listed
        .iter()
        .map(|p| {
            format!(
                r#"<a class="dev-bot-card" href="/bots/{id}">{icon}<div class="dev-bot-name">{name}</div></a>"#,
                id = p.id.0,
                icon = bot_icon(p.id.0, 48),
                name = escape(&p.name),
            )
        })
        .collect();
    let empty = if bot_count == 0 {
        r#"<p class="faint section">This developer has not published any bots yet.</p>"#
    } else {
        ""
    };

    let bio_html = profile.bio.as_deref().map_or(String::new(), |bio| {
        format!(r#"<p class="dev-bio">{}</p>"#, escape(bio))
    });
    let links_html: String = [
        profile.github_url.as_deref().map(|u| {
            format!(
                r#"<a class="dev-link" href="{}" rel="noopener noreferrer">GitHub</a>"#,
                escape(u)
            )
        }),
        profile.twitter_url.as_deref().map(|u| {
            format!(
                r#"<a class="dev-link" href="{}" rel="noopener noreferrer">Twitter/X</a>"#,
                escape(u)
            )
        }),
        profile.website_url.as_deref().map(|u| {
            format!(
                r#"<a class="dev-link" href="{}" rel="noopener noreferrer">Website</a>"#,
                escape(u)
            )
        }),
    ]
    .iter()
    .filter_map(|l| l.clone())
    .collect::<Vec<_>>()
    .join(" ");

    let stats_html = format!(
        r#"<p class="faint">{bot_count} bot{bs} · {server_count} server{ss}</p>"#,
        bs = if bot_count == 1 { "" } else { "s" },
        ss = if server_count == 1 { "" } else { "s" },
    );

    let profile_link = if auth.user_id == dev_id {
        r#" <a class="btn btn-xs" href="/account/profile">Edit profile</a>"#
    } else {
        ""
    };

    let body = format!(
        r#"<div class="page"><div class="dev-header">{badge}<div><h1>{name}{profile_link}</h1>{stats_html}{bio_html}{links_html}</div></div>
<div class="dev-bots">{cards}{empty}</div></div>"#,
        badge = icons::account_badge(dev_id, "lg"),
        name = escape(&profile.username),
    );
    page(
        Some(&auth),
        &state,
        &format!("{} — Developer", escape(&profile.username)),
        &body,
    )
    .into_response()
}

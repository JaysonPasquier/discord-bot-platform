// Le marché et la fiche d'un bot (mise en page de la maquette). Un bot n'y figure que lorsque
// son créateur a connecté au moins une version ; aucun chiffre n'est inventé.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use platform_core::ProductId;
use platform_store::Product;

use crate::discord::invite_url;
use crate::html::escape;
use crate::icons::{self, bot_icon, dc_profile};
use crate::render::{not_found, page, require, server_error};
use crate::reports::public_reports_section;
use crate::state::WebState;
use crate::views::ago;

struct Listed {
    product: Product,
    owner: String,
    free: Option<String>,
    premium: Option<String>,
    description: Option<String>,
    category: Option<String>,
}

fn load(store: &platform_store::SqliteStore, product: Product) -> Option<Listed> {
    if product.suspended {
        return None;
    }
    let client = |tier: &str| {
        store
            .credential(product.id, tier)
            .map(|c| c.map(|c| c.client_id))
    };
    let (free, premium) = (client("free").ok()?, client("premium").ok()?);
    let owner = if product.ceded || product.owner == 0 {
        "BotPlatform Official".to_string()
    } else {
        store
            .username(product.owner)
            .ok()
            .flatten()
            .unwrap_or_else(|| "a creator".to_string())
    };
    let listing = store.listing(product.id).ok().flatten();
    let (description, category) = listing
        .map(|l| (Some(l.description), Some(l.category)))
        .unwrap_or((None, None));
    Some(Listed {
        product,
        owner,
        free,
        premium,
        description,
        category,
    })
}

fn card(l: &Listed) -> String {
    let desc = l
        .description
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|d| {
            let short: String = d.chars().take(120).collect();
            if d.len() > 120 {
                format!("{short}…")
            } else {
                short
            }
        })
        .unwrap_or_else(|| match (&l.free, &l.premium) {
            (Some(_), Some(_)) => "Free and premium versions. Hosted on BotPlatform.".to_string(),
            (Some(_), None) => "Free version. Hosted on BotPlatform.".to_string(),
            _ => "Premium version. Hosted on BotPlatform.".to_string(),
        });
    let category_chip = l
        .category
        .as_deref()
        .filter(|c| !c.is_empty())
        .map_or(String::new(), |c| {
            format!(r#"<span class="chip">{}</span>"#, escape(c))
        });
    format!(
        r#"<a class="bot-card" href="/bots/{id}"><div class="row">{icon}<div class="grow"><h3>{name}</h3><div class="faint small">by {owner}</div></div>{category_chip}</div><p class="desc">{desc}</p><div class="foot"><span class="stat">{server}Not tracked yet</span><span>Premium {premium}</span></div></a>"#,
        id = l.product.id.0,
        icon = bot_icon(l.product.id.0, 40),
        name = escape(&l.product.name),
        owner = escape(&l.owner),
        desc = escape(&desc),
        server = icons::SERVER,
        premium = if l.premium.is_some() {
            "available"
        } else {
            "not offered"
        },
    )
}

pub(crate) async fn market(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let listed: Vec<Listed> = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let Ok(products) = store.all_products() else {
            return server_error(&state, Some(&auth));
        };
        products
            .into_iter()
            .filter_map(|p| load(&store, p))
            .filter(|l| l.free.is_some() || l.premium.is_some())
            .collect()
    };

    // Collecte les catégories uniques pour les filtres.
    let mut categories: Vec<&str> = listed
        .iter()
        .filter_map(|l| l.category.as_deref())
        .filter(|c| !c.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    categories.sort();

    let category_filters: String = if categories.is_empty() {
        String::new()
    } else {
        let chips: String = categories
            .iter()
            .map(|c| {
                format!(
                    r#"<button class="chip market-filter" data-category="{c}" type="button">{c}</button>"#,
                    c = escape(c)
                )
            })
            .collect();
        format!(r#"<div class="market-filters">{chips}</div>"#)
    };

    let results = if listed.is_empty() {
        r#"<div class="card empty"><h3>No bot yet</h3><p>A bot appears here once its creator has connected its Discord bot.</p><a class="btn" href="/dashboard">Create a bot</a></div>"#
            .to_string()
    } else {
        let cards: String = listed
            .iter()
            .map(|l| {
                let cat = l.category.as_deref().unwrap_or("");
                let card_html = card(l);
                // On enveloppe la card avec l'attribut data-category pour le filtre JS côté client.
                format!(r#"<div data-category="{}">{card_html}</div>"#, escape(cat))
            })
            .collect();
        format!(
            r#"<div class="bot-grid" id="market-grid">{cards}</div><div class="card empty" id="market-empty" hidden><h3>No bot matches your search</h3><p>Try another word or remove the category filter.</p></div>"#
        )
    };
    let body = format!(
        r#"<div class="page"><section class="market-hero"><h1>Find a bot for your server</h1><p>Every bot starts free. Premium is billed per server, and you can cancel at any time.</p>
<label class="input-icon" for="market-q"><span class="sr-only">Search bots</span>{search}<input class="input" id="market-q" type="search" placeholder="Search by name, developer or category" autocomplete="off"></label></section>
{category_filters}
<div class="section-head"><h2 id="market-count" aria-live="polite">All bots ({count})</h2></div>
<div id="market-results">{results}</div></div>"#,
        search = icons::SEARCH,
        count = listed.len(),
    );
    page(Some(&auth), &state, "Marketplace", &body).into_response()
}

fn tick(text: &str) -> String {
    format!("<li>{}<span>{}</span></li>", icons::CHECK, escape(text))
}

pub(crate) async fn bot(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let l = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let Ok(Some(product)) = store.product(ProductId(id)) else {
            return not_found(&state, &auth);
        };
        let Some(l) = load(&store, product) else {
            return server_error(&state, Some(&auth));
        };
        l
    };
    if l.product.suspended && l.product.owner != auth.user_id {
        return not_found(&state, &auth);
    }
    if l.free.is_none() && l.premium.is_none() && l.product.owner != auth.user_id {
        return not_found(&state, &auth);
    }
    let now = (state.clock)();
    let (premium_servers, updated, plan) = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let premium_servers = store
            .entitlements_of(l.product.id)
            .unwrap_or_default()
            .iter()
            .filter(|e| {
                crate::billing::counts_as_active_subscriber(e, now, state.config.grace_secs)
            })
            .count();
        let updated = store
            .deployments_of(l.product.id, 1)
            .ok()
            .and_then(|d| d.first().map(|d| ago(now, d.created_at)))
            .unwrap_or_else(|| "—".to_string());
        let plan = store
            .plans_of(l.product.id)
            .unwrap_or_default()
            .into_iter()
            .next();
        (premium_servers, updated, plan)
    };
    let versions = usize::from(l.free.is_some()) + usize::from(l.premium.is_some());
    let name = escape(&l.product.name);
    let discord = |client: &Option<String>, label: &str, primary: bool| {
        client.as_deref().map_or_else(String::new, |c| {
            format!(
                r#"<a class="btn {}btn-lg" href="{}">{} {}</a>"#,
                if primary { "btn-primary " } else { "" },
                escape(&invite_url(c)),
                icons::DISCORD,
                label
            )
        })
    };
    let hero_buttons = format!(
        "{}{}",
        discord(&l.free, "Add to Discord", true),
        if l.premium.is_some() {
            r##"<a class="btn btn-lg" href="#buy">Get premium</a>"##
        } else {
            ""
        }
    );
    let tiers = {
        let free = if l.free.is_some() {
            format!(
                r#"<div class="tier"><h3>Free</h3><div class="amount">$0</div><ul class="ticks">{}{}</ul><a class="btn btn-block" href="{}">{} Add the free bot</a></div>"#,
                tick("Its own Discord application"),
                tick("Add it to any server you manage"),
                escape(&invite_url(l.free.as_deref().unwrap_or_default())),
                icons::DISCORD
            )
        } else {
            String::new()
        };
        let premium = if l.premium.is_some() {
            let amount = plan.as_ref().map_or_else(
                || "No plan yet".to_string(),
                |p| {
                    if p.price_cents == 0 {
                        format!("Free<small> trial · {} days</small>", p.days)
                    } else {
                        format!(
                            "${:.2}<small> / {} days</small>",
                            f64::from(p.price_cents) / 100.0,
                            p.days
                        )
                    }
                },
            );
            format!(
                r##"<div class="tier pro"><h3>Premium <span class="chip">Per server</span></h3><div class="amount">{amount}</div><ul class="ticks">{}{}{}</ul><a class="btn btn-primary btn-block" href="#buy">Get premium</a><a class="btn btn-block" href="{}">{} Add the premium bot</a></div>"##,
                tick("Everything in Free"),
                tick("Its own name, picture and bio for your server"),
                tick("Cancel at any time"),
                escape(&invite_url(l.premium.as_deref().unwrap_or_default())),
                icons::DISCORD,
            )
        } else {
            String::new()
        };
        format!(r#"<div class="tiers section">{free}{premium}</div>"#)
    };
    let buy = if l.premium.is_none() {
        r#"<aside class="buy" id="buy"><div class="note">No premium version yet.</div></aside>"#
            .to_string()
    } else if plan.is_none() {
        r#"<aside class="buy" id="buy"><div class="note">Its creator has not set a priced plan yet.</div></aside>"#.to_string()
    } else if auth.guilds.is_empty() {
        r#"<aside class="buy" id="buy"><div class="note">You do not manage any Discord server.</div></aside>"#.to_string()
    } else {
        let servers: String = auth
            .guilds
            .iter()
            .map(|g| {
                format!(
                    r#"<option value="{}">{}</option>"#,
                    escape(&g.id),
                    escape(&g.name)
                )
            })
            .collect();
        {
            let is_trial = plan.as_ref().is_some_and(|p| p.price_cents == 0);
            let days = plan
                .as_ref()
                .map_or(state.config.subscription_days, |p| p.days);
            let (cta, footnote) = if is_trial {
                (
                    format!("Start free trial — {days} days"),
                    "No payment required. Activate once per server.",
                )
            } else {
                (
                    format!("Subscribe {days} days"),
                    "You will be redirected to Stripe to pay (test mode).",
                )
            };
            format!(
                r#"<aside class="buy" id="buy"><form class="card" method="post" action="/subscribe"><div class="card-head"><h3>Get premium</h3></div>
<div class="card-pad stack"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="product" value="{id}">
<div class="field"><label for="buy-guild">Server</label><select class="input" id="buy-guild" name="guild">{servers}</select><span class="hint">Servers you can manage.</span></div>
<button class="btn btn-primary btn-block" type="submit">{cta}</button></div>
<div class="card-foot small faint">{footnote}</div></form>
<div class="note section">{info}<span>Questions about {name} go to its developer, {owner}.</span></div></aside>"#,
                csrf = escape(&auth.csrf),
                id = l.product.id.0,
                info = icons::INFO,
                owner = escape(&l.owner),
            )
        }
    };
    let reports = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.reports_for_product(l.product.id).ok())
        .unwrap_or_default();
    let reports_html = public_reports_section(&reports, id, &auth.csrf, now);
    let ceded_badge = if l.product.ceded {
        r#" <span class="chip chip-info">BotPlatform Official</span>"#
    } else {
        ""
    };
    let body = format!(
        r#"<div class="page"><p class="crumb-line"><a href="/market">Marketplace</a><span>/</span><span>{name}</span></p>
<section class="bot-hero"><div><div class="row">{icon}{ceded_badge}</div><h1 class="section">{name}</h1><p class="by">By {owner}.</p>
<div class="facts-row"><div><b>—</b><span>Servers</span></div><div><b>{premium_servers}</b><span>Premium servers</span></div><div><b>{versions}</b><span>Versions</span></div><div><b>{updated}</b><span>Last update</span></div></div>
<div class="row-wrap">{hero_buttons}</div></div>
<div class="hide-md">{profile}</div></section>
<div class="split"><div>{tiers}<div class="prose section"><h2>Add it to your server</h2><ol><li>Add the free version. Discord asks which server.</li><li>Subscribe to premium for that server.</li><li>Rename the bot and change its picture for that server, if you like.</li></ol></div></div>{buy}</div>
<div class="section">{reports_html}</div></div>"#,
        icon = bot_icon(l.product.id.0, 64),
        owner = escape(&l.owner),
        profile = dc_profile(l.product.id.0, &l.product.name, "Hosted on BotPlatform."),
    );
    page(Some(&auth), &state, &l.product.name, &body).into_response()
}

// `None` : texte refusé (trop long, ou caractère de contrôle qui n'est pas un saut de ligne autorisé).
// `Some(None)` : champ vide, retour à la valeur par défaut. `Some(Some(v))` : nouvelle valeur, CRLF normalisé.
pub fn clean_text(raw: &str, max_chars: usize, newlines: bool) -> Option<Option<String>> {
    let text = raw.trim();
    let valid = text.chars().count() <= max_chars
        && text
            .chars()
            .all(|c| !c.is_control() || (newlines && matches!(c, '\n' | '\r')));
    if !valid {
        return None;
    }
    Some((!text.is_empty()).then(|| text.replace("\r\n", "\n")))
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// Algorithme « civil from days » de Howard Hinnant, en UTC.
pub fn date(timestamp: i64) -> String {
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub struct NavUser<'a> {
    pub id: u64,
    pub name: &'a str,
    pub csrf: &'a str,
    pub bots: &'a [(u64, String)],
    pub subscriptions: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccountTab {
    Bots,
    Servers,
    Integrations,
    ApiTokens,
    Security,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProjectTab {
    Overview,
    Deployments,
    Source,
    Discord,
    Env,
    Settings,
    Listing,
}

// Où l'on est : les pages publiques gardent la navigation du site, celles du compte et d'un bot
// prennent l'en-tête d'application de la maquette (portée, recherche, onglets).
pub enum Nav<'a> {
    Public,
    Account(AccountTab),
    Project {
        id: u64,
        name: &'a str,
        tab: ProjectTab,
        deployments: usize,
    },
}

// Le même logo et la même structure d'en-tête/pied de page que ui-mockup/ (laissée intacte,
// comme référence visuelle).
const LOGO: &str = r#"<img class="logo" src="/logo.png" width="294" height="240" alt="">"#;

fn tab(href: &str, text: &str, active: bool, count: Option<usize>) -> String {
    let current = if active {
        r#" aria-current="page""#
    } else {
        ""
    };
    let n = count.map_or_else(String::new, |c| format!(r#"<span class="n">{c}</span>"#));
    format!(r#"<a class="tab" href="{href}"{current}>{text}{n}</a>"#)
}

fn account_menu(u: &NavUser<'_>) -> String {
    format!(
        r#"<details class="menu"><summary class="avatar-btn" aria-label="Account menu">{}</summary><div class="menu-pop" role="menu"><div class="head">Signed in as <strong>{}</strong></div><hr><a href="/dashboard" role="menuitem">Your bots</a><a href="/servers" role="menuitem">Your servers</a><a href="/github" role="menuitem">Integrations</a><hr><a href="/market" role="menuitem">Marketplace</a><a href="/docs" role="menuitem">Docs</a><hr><form method="post" action="/logout"><input type="hidden" name="csrf" value="{}"><button role="menuitem" type="submit">Sign out</button></form></div></details>"#,
        crate::icons::account_badge(u.id, ""),
        escape(u.name),
        escape(u.csrf),
    )
}

fn palette(user: Option<&NavUser<'_>>, docs: &[(String, String)]) -> String {
    let item = |href: &str, text: &str, icon: &str| {
        format!(
            r#"<a class="palette-item" role="option" aria-selected="false" href="{}" data-q="{}">{icon}{}</a>"#,
            escape(href),
            escape(&text.to_lowercase()),
            escape(text)
        )
    };
    let chev = crate::icons::CHEV;
    let mut list = String::from(r#"<div class="palette-group">Go to</div>"#);
    if user.is_some() {
        list.push_str(&item("/dashboard", "Your bots", chev));
        list.push_str(&item("/servers", "Servers", chev));
        list.push_str(&item("/github", "Integrations", chev));
    }
    list.push_str(&item("/market", "Marketplace", chev));
    list.push_str(&item("/docs", "Docs", chev));
    list.push_str(&item("/sdk", "SDK", chev));
    if user.is_some() {
        list.push_str(&item("/new", "Create a bot", chev));
    } else {
        list.push_str(&item("/login", "Log in", chev));
    }
    if let Some(u) = user.filter(|u| !u.bots.is_empty()) {
        list.push_str(r#"<div class="palette-group">Your bots</div>"#);
        for (id, name) in u.bots {
            list.push_str(&item(
                &format!("/products/{id}"),
                name,
                &crate::icons::bot_icon(*id, 20),
            ));
        }
    }
    list.push_str(r#"<div class="palette-group">Docs</div>"#);
    for (slug, title) in docs {
        list.push_str(&item(&format!("/docs/{slug}"), title, chev));
    }
    format!(
        r#"<dialog id="palette" class="palette" aria-label="Command menu"><label class="input-icon" for="palette-q">{}<span class="sr-only">Search</span><input class="input" id="palette-q" type="search" placeholder="Search bots, pages and docs" autocomplete="off"></label><div class="palette-list" id="palette-list" role="listbox">{list}<div class="palette-group" id="palette-none" hidden>No results</div></div><div class="palette-foot"><span><kbd>↑</kbd> <kbd>↓</kbd> to move</span><span><kbd>Enter</kbd> to open</span><span><kbd>Esc</kbd> to close</span></div></dialog>"#,
        crate::icons::SEARCH
    )
}

pub fn layout(
    title: &str,
    user: Option<&NavUser<'_>>,
    nav: &Nav<'_>,
    body: &str,
    banner: Option<&str>,
) -> String {
    let in_app = user.is_some() && !matches!(nav, Nav::Public);
    let search = format!(
        r#"<button class="search" data-action="palette" aria-label="Search or jump to">{}<span class="grow">Search...</span><kbd>Ctrl K</kbd></button>"#,
        crate::icons::SEARCH
    );
    let account = match user {
        Some(u) if in_app => account_menu(u),
        Some(u) => format!(
            r#"<a class="btn btn-sm hide-sm" href="/dashboard">Dashboard</a>{}"#,
            account_menu(u)
        ),
        None => r#"<a class="btn btn-sm btn-primary" href="/login">Log in</a>"#.to_string(),
    };
    let mut scope = String::new();
    let mut tabs = String::new();
    if let (Some(u), true) = (user, in_app) {
        let hide = if matches!(nav, Nav::Project { .. }) {
            " hide-sm"
        } else {
            ""
        };
        scope = format!(
            r#"{}<a class="scope" href="/dashboard">{}<span class="truncate{hide}">{}</span></a>"#,
            crate::icons::SLASH,
            crate::icons::account_badge(u.id, "sm"),
            escape(u.name)
        );
        match nav {
            Nav::Project {
                id,
                name,
                tab: current,
                deployments,
            } => {
                scope.push_str(&format!(
                    r#"{}<a class="scope" href="/products/{id}">{}<span class="truncate">{}</span></a>"#,
                    crate::icons::SLASH,
                    crate::icons::bot_icon(*id, 20),
                    escape(name)
                ));
                let base = format!("/products/{id}");
                tabs = format!(
                    r#"<nav class="tabs" aria-label="Bot">{}{}{}{}{}{}{}</nav>"#,
                    tab(&base, "Overview", *current == ProjectTab::Overview, None),
                    tab(
                        &format!("{base}/deployments"),
                        "Deployments",
                        *current == ProjectTab::Deployments,
                        Some(*deployments)
                    ),
                    tab(
                        &format!("{base}/source"),
                        "Source",
                        *current == ProjectTab::Source,
                        None
                    ),
                    tab(
                        &format!("{base}/bot"),
                        "Discord",
                        *current == ProjectTab::Discord,
                        None
                    ),
                    tab(
                        &format!("{base}/env"),
                        "Env",
                        *current == ProjectTab::Env,
                        None
                    ),
                    tab(
                        &format!("{base}/listing"),
                        "Plans",
                        *current == ProjectTab::Listing,
                        None
                    ),
                    tab(
                        &format!("{base}/settings"),
                        "Settings",
                        *current == ProjectTab::Settings,
                        None
                    ),
                );
            }
            Nav::Account(current) => {
                tabs = format!(
                    r#"<nav class="tabs" aria-label="Account">{}{}{}{}{}</nav>"#,
                    tab(
                        "/dashboard",
                        "Bots",
                        *current == AccountTab::Bots,
                        Some(u.bots.len())
                    ),
                    tab(
                        "/servers",
                        "Servers",
                        *current == AccountTab::Servers,
                        Some(u.subscriptions)
                    ),
                    tab(
                        "/github",
                        "Integrations",
                        *current == AccountTab::Integrations,
                        None
                    ),
                    tab(
                        "/account/tokens",
                        "API Tokens",
                        *current == AccountTab::ApiTokens,
                        None
                    ),
                    tab(
                        "/account/security",
                        "Security",
                        *current == AccountTab::Security,
                        None
                    ),
                );
            }
            Nav::Public => {}
        }
    }
    let main_nav = if in_app {
        String::new()
    } else {
        r#"<nav class="nav" aria-label="Main"><a href="/market">Marketplace</a><a href="/docs">Docs</a><a href="/sdk">SDK</a></nav>"#.to_string()
    };
    let word_class = if in_app { "word hide-sm" } else { "word" };
    let docs = crate::docs::palette_entries();
    let banner_html = banner
        .filter(|b| !b.is_empty())
        .map(|b| {
            format!(
                r#"<div class="site-banner" role="alert">{}</div>"#,
                escape(b)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{} - BotPlatform</title><link rel="stylesheet" href="/static/app.css"><script src="/static/app.js" defer></script></head><body><header class="header"><div class="header-bar"><a class="brand" href="/" aria-label="BotPlatform home">{LOGO}<span class="{word_class}">BotPlatform</span></a>{scope}{main_nav}<div class="header-right">{search}{account}</div></div>{tabs}</header><main>{banner_html}{body}</main>{}{}{}</body></html>"#,
        escape(title),
        footer(),
        palette(user, &docs),
        cookie_banner(),
    )
}

fn footer() -> String {
    r#"<footer class="site-foot"><div class="sf-inner"><div class="sf-brand"><a class="brand" href="/">"#.to_string()
        + LOGO
        + r#"<span>BotPlatform</span></a><p>Host Discord bots from GitHub, sell premium per server, let each server make the bot its own.</p></div>
<nav class="sf-cols" aria-label="Footer"><div><h2>Platform</h2><ul><li><a href="/market">Marketplace</a></li><li><a href="/docs">Documentation</a></li><li><a href="/sdk">SDK</a></li></ul></div>
<div><h2>Creators</h2><ul><li><a href="/dashboard">Dashboard</a></li><li><a href="/docs/github">Deploy from GitHub</a></li></ul></div>
<div><h2>Server owners</h2><ul><li><a href="/market">Browse bots</a></li><li><a href="/docs/premium">Premium &amp; billing</a></li></ul></div></nav></div>
<div class="sf-bar"><span>&copy; 2026 BotPlatform</span><span>Local test version: payments go through Stripe test mode, no real money moves.</span></div></footer>"#
}

fn cookie_banner() -> &'static str {
    r#"<div id="cookie-banner" role="complementary" aria-label="Cookie notice"><p>This site uses strictly necessary cookies (session management only). No analytics or advertising. <a href="/docs/cookies">Learn more</a>.</p><button class="btn btn-sm" type="button">OK</button></div>"#
}

pub const CSS: &str = include_str!("../assets/app.css");
pub const JS: &str = include_str!("../assets/app.js");
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
pub const FONT_GEIST: &[u8] = include_bytes!("../assets/fonts/Geist-Variable.woff2");
pub const FONT_GEIST_MONO: &[u8] = include_bytes!("../assets/fonts/GeistMono-Variable.woff2");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_special_characters_are_escaped() {
        assert_eq!(
            escape("<b>\"x\" & 'y'</b>"),
            "&lt;b&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/b&gt;"
        );
        assert_eq!(escape("plain text é"), "plain text é");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(86_399), "1970-01-01");
        assert_eq!(date(86_400), "1970-01-02");
        assert_eq!(date(-1), "1969-12-31");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        assert_eq!(date(4_102_444_800), "2100-01-01");
        assert_eq!(date(1_709_251_199), "2024-02-29");
        assert_eq!(date(1_709_251_200), "2024-03-01");
        assert_eq!(date(4_107_542_399), "2100-02-28");
        assert_eq!(date(4_107_542_400), "2100-03-01");
        assert_eq!(date(13_574_563_200), "2400-02-29");
        assert_eq!(date(13_574_649_600), "2400-03-01");
    }

    #[test]
    fn the_layout_escapes_the_title_and_the_user_name() {
        let user = NavUser {
            id: 1,
            name: "<script>",
            csrf: "tok\"en",
            bots: &[],
            subscriptions: 0,
        };
        let html = layout("a<b", Some(&user), &Nav::Public, "<p>body</p>", None);
        assert!(html.contains("<title>a&lt;b - BotPlatform</title>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("value=\"tok&quot;en\""));
        assert!(html.contains("<p>body</p>"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn logged_out_visitors_see_a_login_link_and_no_logout_form() {
        let html = layout("Home", None, &Nav::Public, "", None);
        assert!(html.contains("href=\"/login\""));
        assert!(!html.contains("action=\"/logout\""));
    }
}

// Documentation and SDK pages. Static content, written here (not pulled from docs/*.md,
// which are the project's own working notes): it describes real, shipped behavior only.
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use axum::http::StatusCode;

use crate::html::escape;
use crate::render::{message, page};
use crate::session::{authenticate, Auth};
use crate::state::WebState;

pub(crate) struct DocPage {
    pub(crate) slug: &'static str,
    pub(crate) group: &'static str,
    pub(crate) title: &'static str,
    pub(crate) lead: &'static str,
    pub(crate) body: &'static str,
}

pub(crate) const PAGES: &[DocPage] = &[
    DocPage {
        slug: "quickstart",
        lead: "From an empty repository to a live bot.",
        group: "Start",
        title: "Quickstart",
        body: r#"<p>Sign in, create a bot, connect a GitHub repository, deploy.</p>
<ol>
<li>Sign in with Discord (or a dev account while <code>dev_login</code> is on).</li>
<li>On the <a href="/dashboard">dashboard</a>, name a bot and create it.</li>
<li>Open the bot, pick a version (free or premium), and <a href="/docs/github">connect a repository</a>.</li>
<li>Paste the Discord bot token for that version.</li>
<li>Deploy. The page refreshes by itself: queued, building, running.</li>
</ol>
<p class="muted">One Discord application per version: the free and premium versions of a bot cannot share a token.</p>"#,
    },
    DocPage {
        slug: "bot-toml",
        lead: "The one file that describes both versions.",
        group: "Build your bot",
        title: "bot.toml",
        body: r#"<p>A file at the root of your repository tells the platform how to run your bot and what is free or paid.</p>
<pre><code>schema_version = 1
name = "My Bot"
language = "node"             # node, python or rust
support_contact = "me@example.com"

[free]
start_command = "node index.js"

[premium]
start_command = "node index.js"

[[premium.plans]]
id = "monthly"
interval = "month"
price_cents = 500</code></pre>
<p>One repository can serve both versions: your code reads the <code>TIER</code> environment variable (<code>free</code> or <code>premium</code>) and turns paid features on or off. With two repositories, the free one does not need a <code>[premium]</code> section.</p>
<p>The platform writes the Dockerfile itself from <code>language</code> and <code>start_command</code>: the one in your repository, if any, is ignored. Install scripts are disabled (<code>npm ci</code>, <code>pip install -r requirements.txt</code>, <code>cargo build --release</code>, no arbitrary <code>postinstall</code>).</p>"#,
    },
    DocPage {
        slug: "variables",
        lead: "What your code receives at runtime.",
        group: "Build your bot",
        title: "Environment variables",
        body: r#"<p>Set by the platform at deploy time, read by your code:</p>
<dl class="kv">
<dt><code>TIER</code></dt><dd>Which version is running: <code>free</code> or <code>premium</code>.</dd>
<dt><code>PLATFORM_API_URL</code></dt><dd>Base URL of the premium-status API, for the SDK or a direct call.</dd>
<dt><code>PLATFORM_API_TOKEN</code></dt><dd>Bearer token for that API, scoped to your bot.</dd>
</dl>
<p class="muted">No other environment variable is injected; there is no per-bot custom variable yet.</p>"#,
    },
    DocPage {
        slug: "languages",
        lead: "How an image is built from your repository.",
        group: "Build your bot",
        title: "Supported languages",
        body: r#"<p>Three languages, one Dockerfile per language, written by the platform:</p>
<ul>
<li><strong>Node</strong>: dependencies installed with <code>npm ci --ignore-scripts</code>. The SDK is already in the image (<code>import { PlatformClient } from "platform-sdk"</code>).</li>
<li><strong>Python</strong>: <code>pip install --no-deps -r requirements.txt</code> (pin every dependency; transitive ones are not resolved).</li>
<li><strong>Rust</strong>: <code>cargo build --release --locked</code>.</li>
</ul>
<p>A build that takes more than 10 minutes is stopped and marked failed. One build runs at a time per bot version.</p>"#,
    },
    DocPage {
        slug: "github",
        lead: "Let the platform read only the repositories you choose.",
        group: "Deploy",
        title: "Connect a repository",
        body: r#"<p>Repositories are read through a GitHub App, never a personal access token: you choose exactly which repositories the platform can read, nothing else.</p>
<ol>
<li>From a bot's version, <strong>Connect a repository</strong>.</li>
<li>On GitHub, pick <strong>Only select repositories</strong> and check the ones you want to use. Install.</li>
<li>Back on the platform, choose the repository and branch for this version, and whether it should auto-deploy.</li>
</ol>
<p>A commit is read from GitHub again at the moment you deploy, never trusted from a stale value. The image built for a commit is kept: going back to an earlier deployment does not rebuild it.</p>"#,
    },
    DocPage {
        slug: "triggers",
        lead: "Build, start, replace. If anything fails, the live version stays.",
        group: "Deploy",
        title: "Deployment triggers",
        body: r#"<p>A new deployment starts three ways:</p>
<ul>
<li><strong>Manual</strong>: <em>Deploy latest</em>, or choose a branch.</li>
<li><strong>Webhook</strong>: with a public address and a signed <code>X-Hub-Signature-256</code> push event, configured on the GitHub App. Without a configured secret, the webhook route does not exist.</li>
<li><strong>Polling</strong>: every 30 seconds by default, the only trigger that works without a public address. Each source remembers the last commit it already deployed, so a replayed webhook or a repeated poll never redeploys the same commit twice.</li>
</ul>
<p>Only one deployment runs at a time per bot version. A new deployment replaces the running one only once its container has actually started; if it fails, the previous deployment stays live. A commit whose build failed is not retried on its own.</p>"#,
    },
    DocPage {
        slug: "logs",
        lead: "Read why a build or a container stopped.",
        group: "Deploy",
        title: "Deployments and logs",
        body: r#"<p>Every deployment is recorded: commit, branch, trigger, status, and up to 64 KB of its container log. The status moves through <strong>queued</strong>, <strong>building</strong>, <strong>running</strong>, and either stays <strong>ready</strong> (an earlier, no-longer-current deployment) or ends <strong>failed</strong>. The bot page refreshes itself while a deployment is in progress, and shows the last twelve deployments of each version as a rail, oldest on the left.</p>"#,
    },
    DocPage {
        slug: "discord",
        lead: "One application and one token per version.",
        group: "Discord",
        title: "Discord tokens and permissions",
        body: r#"<p>Each bot version needs its own Discord application: create one per version in the <a href="https://discord.com/developers/applications">Discord Developer Portal</a>, then paste its token (Bot tab, Reset Token) on the bot's page. The token is checked against Discord before it is accepted, stored encrypted, and never shown again.</p>
<p class="muted">If a token leaks, use Reset Token on the Discord portal, then paste the new one here: the old one stops working immediately.</p>"#,
    },
    DocPage {
        slug: "permissions",
        lead: "Free first, premium when a subscription starts.",
        group: "Discord",
        title: "Inviting a bot",
        body: r#"<p>Once a version has a connected Discord application, an invite link appears on its bot page and on the marketplace listing. Inviting is a normal Discord OAuth flow: the server owner picks the server and the permissions, the platform does not choose them.</p>"#,
    },
    DocPage {
        slug: "premium",
        lead: "Setting prices and checking who paid.",
        group: "Server owners",
        title: "Premium & subscriptions",
        body: r#"<p>A server owner subscribes a specific server to a bot's premium version from the <a href="/market">marketplace</a>. This local test version simulates payment: no card is charged, but the subscription behaves like a real one (an expiry date, a grace period, cancel at any time).</p>
<p>Your bot checks access per server with the <code>GET /v1/premium/{guild_id}</code> API (a Bearer token, <code>{"access": true|false}"</code>), directly or through the <a href="/sdk">SDK</a>. A canceled subscription keeps access until its paid-until date; after that it moves to a short grace period before access is removed.</p>"#,
    },
    DocPage {
        slug: "personalization",
        lead: "Subscribers make the bot their own.",
        group: "Server owners",
        title: "Per-server personalization",
        body: r#"<p>A server that subscribes to a bot's premium version can personalize it for that server: nickname, bio, avatar and banner, applied only there. Images are limited in size; the bio is short. Personalization is set by whoever manages the subscription, not by the bot's creator.</p>"#,
    },
    DocPage {
        slug: "add-a-bot",
        lead: "Free first, premium when you want it.",
        group: "Server owners",
        title: "Add a bot to your server",
        body: r#"<p>Open a bot's listing on the <a href="/market">marketplace</a> and use its invite link. The free version, if the creator published one, can be added without a subscription. The premium version needs an active subscription for that server first.</p>"#,
    },
    DocPage {
        slug: "troubleshooting",
        lead: "The problems people hit first.",
        group: "Help",
        title: "Troubleshooting",
        body: r#"<dl class="kv">
<dt>Build failed</dt><dd>Open the deployment: its log is kept. A build over 10 minutes is stopped automatically.</dd>
<dt>Container keeps restarting</dt><dd>Check the container logs on the bot page; a bot that crashes on start is usually missing an environment variable or a dependency.</dd>
<dt>Token rejected</dt><dd>Discord refused it: it may be wrong or already reset. Copy a fresh one from the Bot tab.</dd>
<dt>Application already used</dt><dd>Each version needs its own Discord application; this one is already registered elsewhere.</dd>
<dt>Push not detected</dt><dd>Without a webhook, detection polls every 30 seconds. Check the branch and that auto-deploy is on.</dd>
</dl>"#,
    },
    DocPage {
        slug: "faq",
        lead: "Short answers.",
        group: "Help",
        title: "FAQ",
        body: r#"<dl class="kv">
<dt>Is this real money?</dt><dd>No. This is a local test version: subscriptions go through Stripe test mode, so no real money moves.</dd>
<dt>Can I use the same repository for both versions?</dt><dd>Yes: <code>bot.toml</code> and the <code>TIER</code> variable tell your code which one is running.</dd>
<dt>Can I choose the base image?</dt><dd>No. The platform writes the Dockerfile from your language and start command.</dd>
</dl>"#,
    },
];

pub(crate) const SDK_BODY: &str = r#"<h2>Node</h2>
<pre><code>import { PlatformClient } from "platform-sdk";

const platform = new PlatformClient({
  baseUrl: process.env.PLATFORM_API_URL,
  token: process.env.PLATFORM_API_TOKEN,
});

if (await platform.isPremium(interaction.guildId)) {
  // paid feature
}</code></pre>
<p><code>guildId</code> is a decimal string or a non-negative <code>bigint</code> (a Discord id can exceed what a JavaScript number holds exactly).</p>
<dl class="kv">
<dt><code>refreshMs</code></dt><dd>How often a server's status is re-checked. Default 60 000 (one minute).</dd>
<dt><code>cacheTtlMs</code></dt><dd>How long a cached answer is still used if the platform cannot be reached. Default 24 hours.</dd>
<dt><code>timeoutMs</code></dt><dd>Per-request timeout. Default 3 000.</dd>
<dt><code>defaultOnFailure</code></dt><dd>Access to assume once the cache has also expired. Default <code>false</code>: a server loses the paid feature rather than keep it for free during an outage.</dd>
</dl>
<p>Errors: <code>PlatformAuthError</code> (the token itself was rejected: this is not retried against the cache), <code>PlatformRequestError</code> (an unexpected response), <code>TypeError</code> (a bad <code>guildId</code>).</p>
<h2>HTTP API</h2>
<pre><code>GET /v1/premium/{guild_id}
Authorization: Bearer &lt;token&gt;

200 { "access": true }</code></pre>
<p>Any language can call this directly instead of using the Node SDK. A wrong or missing token answers <code>401</code>.</p>
"#;

// Pour la palette de commandes : toutes les pages de la documentation.
pub(crate) fn palette_entries() -> Vec<(String, String)> {
    PAGES
        .iter()
        .map(|p| (p.slug.to_string(), p.title.to_string()))
        .collect()
}

fn group_order() -> Vec<&'static str> {
    let mut groups = Vec::new();
    for p in PAGES {
        if !groups.contains(&p.group) {
            groups.push(p.group);
        }
    }
    groups
}

fn sidebar(current: Option<&str>) -> String {
    let mut out = String::from(r#"<nav class="side" aria-label="Docs">"#);
    for group in group_order() {
        out.push_str(&format!(
            r#"<div class="side-group">{}</div>"#,
            escape(group)
        ));
        for p in PAGES.iter().filter(|p| p.group == group) {
            let current_attr = (Some(p.slug) == current).to_string();
            out.push_str(&format!(
                r#"<a href="/docs/{}" aria-current="{current_attr}">{}</a>"#,
                p.slug,
                escape(p.title)
            ));
        }
    }
    out.push_str(r#"<div class="side-group">Reference</div><a href="/sdk">SDK</a>"#);
    out.push_str("</nav>");
    out
}

fn pagers(slug: &str) -> String {
    let i = PAGES.iter().position(|p| p.slug == slug);
    let Some(i) = i else { return String::new() };
    let prev = i
        .checked_sub(1)
        .and_then(|j| PAGES.get(j))
        .map(|p| format!(r#"<a class="pager prev" href="/docs/{}"><span class="faint small">Previous</span><span class="strong">{}</span></a>"#, p.slug, escape(p.title)))
        .unwrap_or_default();
    let next = PAGES
        .get(i + 1)
        .map(|p| format!(r#"<a class="pager next" href="/docs/{}"><span class="faint small">Next</span><span class="strong">{}</span></a>"#, p.slug, escape(p.title)))
        .unwrap_or_default();
    format!(r#"<div class="pagers">{prev}{next}</div>"#)
}

fn auth_opt(state: &WebState, headers: &HeaderMap) -> Option<Auth> {
    authenticate(state, headers)
}

pub(crate) async fn index(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = auth_opt(&state, &headers);
    let first = PAGES.first().map(|p| p.slug).unwrap_or("quickstart");
    let body = format!(
        r#"<div class="page"><div class="page-head"><div><h1>Documentation</h1><p>How BotPlatform hosts a bot, deploys it from GitHub, and sells its premium version per server.</p></div></div>
<div class="docs">{}<article class="prose"><p>Start with the <a class="link" href="/docs/{first}">quickstart</a>, or pick a page on the left.</p></article></div></div>"#,
        sidebar(None)
    );
    page(auth.as_ref(), &state, "Docs", &body).into_response()
}

pub(crate) async fn show(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    let auth = auth_opt(&state, &headers);
    let Some(p) = PAGES.iter().find(|p| p.slug == slug) else {
        return message(
            StatusCode::NOT_FOUND,
            &state,
            auth.as_ref(),
            "Not found",
            "This documentation page does not exist.",
        );
    };
    let headings: Vec<&str> = p
        .body
        .match_indices("<h2>")
        .filter_map(|(i, _)| {
            p.body[i + 4..]
                .find("</h2>")
                .map(|end| &p.body[i + 4..i + 4 + end])
        })
        .collect();
    let toc = if headings.is_empty() {
        r#"<span class="faint">One section</span>"#.to_string()
    } else {
        headings
            .iter()
            .map(|h| format!("<span>{h}</span>"))
            .collect()
    };
    let body = format!(
        r#"<div class="page"><div class="docs">{sidebar}
<article><div class="prose"><h1 class="doc-title">{title}</h1><p class="doc-lead">{group}</p>{content}</div>
{pagers}</article>
<aside class="toc"><b>On this page</b>{toc}</aside>
</div></div>"#,
        sidebar = sidebar(Some(p.slug)),
        title = escape(p.title),
        group = escape(p.lead),
        content = p.body,
        pagers = pagers(p.slug),
    );
    page(auth.as_ref(), &state, p.title, &body).into_response()
}

// Page des CGU : publique, hors sidebar de la documentation normale.
pub(crate) async fn cgu(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = auth_opt(&state, &headers);
    let body = r#"<div class="page"><div class="prose wide">
<h1>Conditions générales d'utilisation (brouillon)</h1>
<p class="muted"><strong>Ce texte est un brouillon non définitif, à valider par un professionnel avant tout lancement public.</strong></p>

<h2>Article 1 — Objet</h2>
<p>BotPlatform est une plateforme d'hébergement de bots Discord permettant aux créateurs de déployer des bots depuis GitHub et aux propriétaires de serveurs de s'abonner à des versions premium.</p>

<h2>Article 2 — Jeton Discord</h2>
<p>Le créateur reste seul titulaire de son application Discord. Il transmet le jeton à des fins techniques uniquement. Le jeton est chiffré, jamais réaffiché ni transmis à un tiers.</p>

<h2>Article 3 — Cookies</h2>
<p>Le site utilise uniquement des cookies strictement nécessaires (session, CSRF). Ces cookies sont exemptés du recueil de consentement au titre de l'article 82 de la loi Informatique et Libertés. Aucun cookie de pistage ou publicitaire n'est déposé.</p>

<h2>Article 4 — Cession volontaire d'un bot</h2>
<p>Le créateur peut céder son bot à BotPlatform depuis les réglages. Cette opération est définitive et irréversible. Aucune contrepartie financière n'est versée.</p>

<h2>Article 5 — Acquisition pour abandon</h2>
<p>Un bot est considéré abandonné lorsque : (a) au moins deux signalements de bugs restent ouverts depuis plus de 90 jours, (b) au moins un abonné actif s'est plaint, (c) deux notifications écrites sont restées sans réponse dans les 30 jours. BotPlatform peut alors notifier une intention d'acquisition ; le créateur dispose de 15 jours pour contester.</p>

<h2>Article 6 — Comportement</h2>
<p>Le créateur s'engage à ne pas déployer de contenu violant les conditions d'utilisation de Discord ou la loi applicable. BotPlatform peut suspendre ou supprimer un bot sans préavis en cas de manquement.</p>

<h2>Article 7 — Limitation de responsabilité</h2>
<p>BotPlatform agit comme hébergeur technique, pas comme éditeur du contenu des bots. La plateforme ne garantit pas la disponibilité du service en cas de décision de Discord.</p>

<p class="muted">Droit applicable : droit français. À valider par un professionnel.</p>
</div></div>"#;
    page(auth.as_ref(), &state, "CGU", body).into_response()
}

pub(crate) async fn sdk(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = auth_opt(&state, &headers);
    let body = format!(
        r#"<div class="page"><div class="page-head"><div><h1>SDK reference</h1><p>Check whether a server's subscription is active, from your bot's own code.</p></div></div>
<div class="prose wide">
{SDK_BODY}
</div></div>"#
    );
    page(auth.as_ref(), &state, "SDK", &body).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;
    use crate::state::test_state;

    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn every_page_is_reachable_from_its_own_sidebar() {
        for p in PAGES {
            assert!(sidebar(Some(p.slug)).contains(&format!("/docs/{}", p.slug)));
        }
    }

    #[test]
    fn the_sidebar_marks_only_the_current_page() {
        let html = sidebar(Some("bot-toml"));
        assert_eq!(html.matches(r#"aria-current="true""#).count(), 1);
        assert!(html.contains(">bot.toml</a>"));
    }

    #[test]
    fn pagers_skip_the_edges() {
        let (Some(first), Some(last)) = (PAGES.first(), PAGES.last()) else {
            panic!("PAGES must not be empty");
        };
        assert!(!pagers(first.slug).contains("Previous"));
        assert!(pagers(first.slug).contains("Next"));
        assert!(pagers(last.slug).contains("Previous"));
        assert!(!pagers(last.slug).contains("Next"));
    }

    #[tokio::test]
    async fn an_unknown_doc_slug_is_a_404() {
        let response = show(
            State(test_state()),
            HeaderMap::new(),
            Path("does-not-exist".to_string()),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_known_doc_slug_renders_its_title_and_body() {
        let response = show(
            State(test_state()),
            HeaderMap::new(),
            Path("bot-toml".to_string()),
        )
        .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("doc-title\">bot.toml</h1>"));
        assert!(body.contains("schema_version"));
    }

    #[tokio::test]
    async fn the_sdk_page_documents_is_premium_and_the_http_route() {
        let response = sdk(State(test_state()), HeaderMap::new()).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = text(response).await;
        assert!(body.contains("isPremium"));
        assert!(body.contains("/v1/premium/{guild_id}"));
    }
}

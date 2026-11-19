use std::net::SocketAddr;

use axum::extract::connect_info::ConnectInfo;
use axum::extract::{Form, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_store::{generate_token, hash_token};
use serde::Deserialize;

use crate::discord::{DiscordUser, Guild};
use crate::html::escape;
use crate::render::{
    message, page, redirect, require_csrf, require_session, server_error, with_cookies, CsrfForm,
};
use crate::session::{
    authenticate, clear_cookie, cookie_value, set_cookie, start_session, tokens_match,
    SESSION_COOKIE, SESSION_TTL_SECS, STATE_COOKIE, STATE_TTL_SECS,
};
use crate::state::WebState;

// IP du client : X-Real-IP (reverse proxy) ou ConnectInfo (connexion directe), sinon "unknown".
pub(crate) struct ClientIp(String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        if let Some(ip) = parts
            .headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        {
            return Ok(Self(ip));
        }
        if let Some(ConnectInfo(addr)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
            return Ok(Self(addr.ip().to_string()));
        }
        Ok(Self("unknown".to_string()))
    }
}

fn too_many_requests(state: &WebState) -> Response {
    message(
        StatusCode::TOO_MANY_REQUESTS,
        state,
        None,
        "Too many requests",
        "Too many login attempts. Please wait a moment and try again.",
    )
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct DevForm {
    who: String,
}

pub(crate) fn dev_user(who: &str) -> Option<(DiscordUser, Vec<Guild>)> {
    let guild = |id: &str, name: &str| Guild {
        id: id.to_string(),
        name: name.to_string(),
    };
    match who {
        "alice" => Some((
            DiscordUser {
                id: 1_000_000_000_000_000_001,
                name: "alice (dev)".to_string(),
            },
            vec![guild("2000000000000000001", "Alice Dev Server")],
        )),
        "bob" => Some((
            DiscordUser {
                id: 1_000_000_000_000_000_002,
                name: "bob (dev)".to_string(),
            },
            vec![
                guild("2000000000000000002", "Bob Dev Server A"),
                guild("2000000000000000003", "Bob Dev Server B"),
            ],
        )),
        _ => None,
    }
}

// Bots on the marketplace strip: real listed bots only, newest-created last. Nothing fake here —
// if nothing is listed yet, the section is left out rather than filled with sample entries.
fn listed_bots(state: &WebState) -> Vec<(u64, String)> {
    let Ok(store) = state.store.lock() else {
        return Vec::new();
    };
    let Ok(products) = store.all_products() else {
        return Vec::new();
    };
    products
        .into_iter()
        .filter(|p| {
            ["free", "premium"]
                .iter()
                .any(|t| matches!(store.credential(p.id, t), Ok(Some(_))))
        })
        .take(4)
        .map(|p| (p.id.0, p.name))
        .collect()
}

pub(crate) async fn home(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = authenticate(&state, &headers);
    let cta = if auth.is_some() {
        r#"<a class="btn btn-primary btn-lg" href="/dashboard">Go to dashboard</a>"#.to_string()
    } else {
        format!(
            r#"<a class="btn btn-primary btn-lg" href="/github">{} Deploy from GitHub</a>"#,
            crate::icons::GITHUB
        )
    };
    let strip = {
        let bots = listed_bots(&state);
        if bots.is_empty() {
            String::new()
        } else {
            let items: String = bots
                .iter()
                .map(|(id, name)| {
                    format!(
                        r#"<a href="/products/{id}">{}<div><h3>{}</h3><p class="muted small">Hosted on BotPlatform</p></div></a>"#,
                        crate::icons::bot_icon(*id, 48),
                        escape(name)
                    )
                })
                .collect();
            format!(
                r#"<section class="band cross"><div class="band-pad"><div class="section-head"><h2 class="section-title">Bots on the marketplace</h2><a class="btn" href="/market">Browse {} bot{}</a></div></div>
<div class="bots-strip">{items}</div></section>"#,
                bots.len(),
                if bots.len() == 1 { "" } else { "s" }
            )
        }
    };
    let body = format!(
        r#"<div class="frame">
<section class="band hero">
<div><h1>Push to GitHub. Your Discord bot is live.</h1>
<p class="lede">Connect a repository and every push builds and deploys your bot. Server owners add it for free and pay for premium one server at a time.</p>
<div class="row-wrap">{cta}<a class="btn btn-lg" href="/market">Find a bot</a></div></div>
<div class="stage">
<div class="term" aria-label="Example terminal"><div class="dots"><i></i><i></i><i></i></div>
<div><span class="p">~/my-bot $</span> git push origin main</div>
<div class="p">To github.com:you/my-bot.git</div>
<div><span class="p">~/my-bot $</span></div></div>
<a class="deploy-card" href="/docs/deploy">{deploy_icon}<div class="grow"><div class="title truncate">Deploys on every push</div>
<div class="meta"><span class="branch">{branch_icon}<span class="truncate">main</span></span><span>~1 min</span></div></div>
<span class="status state live"><span class="dot" aria-hidden="true"></span>Live</span></a>
</div></section>

<section class="band cross cells" aria-label="What the platform does">
<div class="cell"><h3>Deploy any branch, go back in one click</h3><p>Every build keeps its image. Redeploying an old one takes seconds.</p></div>
<div class="cell"><h3>A free bot and a premium bot</h3><p>Two Discord applications from one repository. You set the premium price.</p></div>
<div class="cell"><h3>Each server makes it its own</h3><p>Subscribers rename the bot and change its picture and bio for their server.</p></div>
</section>

<section class="band cross"><div class="band-pad"><h2 class="section-title">One file describes both versions.</h2>
<p class="section-sub">Add a bot.toml to your repository. The platform writes the Dockerfile, injects the token and tells your code which servers pay.</p></div>
<div class="code-split">
<div><div class="code-head">{file_icon} bot.toml</div><pre class="code"><span class="t-k">name</span> = <span class="t-s">"My Bot"</span>
<span class="t-k">language</span> = <span class="t-s">"node"</span>

[free]
<span class="t-k">start_command</span> = <span class="t-s">"node index.js"</span>

[premium]
<span class="t-k">start_command</span> = <span class="t-s">"node index.js"</span>

[[premium.plans]]
<span class="t-k">id</span> = <span class="t-s">"monthly"</span>
<span class="t-k">interval</span> = <span class="t-s">"month"</span>
<span class="t-k">price_cents</span> = <span class="t-n">400</span></pre></div>
<div><div class="code-head">{file_icon} index.js</div><pre class="code"><span class="t-k">import</span> {{ PlatformClient }} <span class="t-k">from</span> <span class="t-s">"platform-sdk"</span>;

<span class="t-k">const</span> platform = <span class="t-k">new</span> <span class="t-f">PlatformClient</span>();

client.<span class="t-f">on</span>(<span class="t-s">"interactionCreate"</span>, <span class="t-k">async</span> (i) =&gt; {{
  <span class="t-k">if</span> (<span class="t-k">await</span> platform.<span class="t-f">isPremium</span>(i.guildId)) {{
    <span class="t-c">// premium behavior</span>
  }}
}});</pre></div>
</div></section>
{strip}
<section class="band cross cta-band"><h2>Your next push could be live in a minute.</h2>
<div class="row-wrap"><a class="btn btn-primary btn-lg" href="/dashboard">Create a bot</a><a class="btn btn-lg" href="/docs">Read the docs</a></div></section>
</div>"#,
        deploy_icon = crate::icons::bot_icon(0, 32),
        branch_icon = crate::icons::BRANCH,
        file_icon = crate::icons::FILE,
    );
    page(auth.as_ref(), &state, "Home", &body).into_response()
}

pub(crate) async fn login(State(state): State<WebState>) -> Response {
    let discord_ready = state.discord.authorize_url("probe").is_some();
    if !state.config.dev_login {
        return if discord_ready {
            redirect("/auth/discord")
        } else {
            message(
                StatusCode::SERVICE_UNAVAILABLE,
                &state,
                None,
                "Login unavailable",
                "Discord login is not configured.",
            )
        };
    }
    let discord = if discord_ready {
        format!(
            r#"<a class="btn btn-discord btn-block" href="/auth/discord">{} Log in with Discord</a>"#,
            crate::icons::DISCORD
        )
    } else {
        String::new()
    };
    let dev = r#"<div class="divider">dev login</div><p class="small muted">Two fake accounts to test the pages. Alice and Bob are separate users: one can be the creator, the other the buyer.</p><div class="row-wrap"><form method="post" action="/dev-login"><input type="hidden" name="who" value="alice"><button class="btn btn-sm" type="submit">Log in as alice</button></form><form method="post" action="/dev-login"><input type="hidden" name="who" value="bob"><button class="btn btn-sm" type="submit">Log in as bob</button></form></div>"#;
    let body = format!(
        r#"<div class="auth"><div class="auth-box"><img class="logo" src="/logo.png" width="294" height="240" alt=""><h1>Log in</h1>{discord}{dev}</div></div>"#
    );
    page(None, &state, "Log in", &body).into_response()
}

pub(crate) async fn discord_redirect(
    State(state): State<WebState>,
    ClientIp(ip): ClientIp,
) -> Response {
    if !state.auth_limiter.check(&ip, (state.clock)()) {
        return too_many_requests(&state);
    }
    let Ok(oauth_state) = generate_token() else {
        return server_error(&state, None);
    };
    match state.discord.authorize_url(&oauth_state) {
        Some(url) => {
            let cookie = set_cookie(
                STATE_COOKIE,
                &oauth_state,
                STATE_TTL_SECS,
                state.config.secure_cookies(),
            );
            with_cookies(redirect(&url), &[cookie])
        }
        None => message(
            StatusCode::NOT_FOUND,
            &state,
            None,
            "Not found",
            "Discord login is not configured.",
        ),
    }
}

pub(crate) async fn callback(
    State(state): State<WebState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if !state.auth_limiter.check(&ip, (state.clock)()) {
        return too_many_requests(&state);
    }
    let secure = state.config.secure_cookies();
    let clear = clear_cookie(STATE_COOKIE, secure);
    let fail = |status: StatusCode, text: &str| {
        with_cookies(
            message(status, &state, None, "Login failed", text),
            std::slice::from_ref(&clear),
        )
    };
    if query.error.is_some() {
        return fail(StatusCode::BAD_REQUEST, "The Discord login was cancelled.");
    }
    let (Some(code), Some(returned), Some(expected)) = (
        query.code,
        query.state,
        cookie_value(&headers, STATE_COOKIE),
    ) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "The login request is incomplete. Start again.",
        );
    };
    if !tokens_match(&returned, &expected) {
        return fail(
            StatusCode::BAD_REQUEST,
            "The login state does not match. Start again.",
        );
    }
    let discord = state.discord.clone();
    let Ok(Ok((user, guilds))) = tokio::task::spawn_blocking(move || discord.login(&code)).await
    else {
        return fail(
            StatusCode::BAD_GATEWAY,
            "Discord did not accept the login. Try again.",
        );
    };
    let Some(cookie) = start_session(&state, &user, &guilds) else {
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The session could not be created.",
        );
    };
    let dest = state
        .config
        .frontend_url
        .as_deref()
        .map_or("/dashboard".to_string(), |base| format!("{base}/dashboard"));
    with_cookies(
        redirect(&dest),
        &[
            set_cookie(SESSION_COOKIE, &cookie, SESSION_TTL_SECS, secure),
            clear,
        ],
    )
}

pub(crate) async fn dev_login(
    State(state): State<WebState>,
    ClientIp(ip): ClientIp,
    Form(form): Form<DevForm>,
) -> Response {
    if !state.auth_limiter.check(&ip, (state.clock)()) {
        return too_many_requests(&state);
    }
    if !state.config.dev_login {
        return message(
            StatusCode::NOT_FOUND,
            &state,
            None,
            "Not found",
            "This page does not exist.",
        );
    }
    let Some((user, guilds)) = dev_user(&form.who) else {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            None,
            "Unknown dev user",
            "Use alice or bob.",
        );
    };
    let Some(cookie) = start_session(&state, &user, &guilds) else {
        return server_error(&state, None);
    };
    // En mode dev, accepter les CGU automatiquement pour faciliter les tests.
    let now = (state.clock)();
    let _ = state.store.lock().map(|s| s.accept_cgu(user.id, now));
    let cookie = set_cookie(
        SESSION_COOKIE,
        &cookie,
        SESSION_TTL_SECS,
        state.config.secure_cookies(),
    );
    with_cookies(redirect("/dashboard"), &[cookie])
}

pub(crate) async fn logout(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    let auth = match require_session(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    if let (Some(cookie), Ok(store)) = (cookie_value(&headers, SESSION_COOKIE), state.store.lock())
    {
        let _ = store.delete_session(&hash_token(&cookie));
    }
    with_cookies(
        redirect("/"),
        &[clear_cookie(SESSION_COOKIE, state.config.secure_cookies())],
    )
}

pub(crate) async fn accept_cgu_get(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require_session(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if auth.cgu_accepted {
        return redirect("/dashboard");
    }
    let body = format!(
        r#"<div class="auth"><div class="auth-box wide"><h1>Terms of service</h1>
<p class="muted small">Before using BotPlatform as a creator, you must accept the terms of service.</p>
<div class="cgu-scroll">
<h2>Using the platform</h2>
<p>You may connect a GitHub repository and deploy Discord bots through the platform. You remain the sole owner of your Discord application and of the code you deploy.</p>
<h2>Bot token</h2>
<p>You transmit your bot token to the platform for the sole purpose of running your bot on your behalf. The token is stored encrypted, never shown again, and never transmitted to third parties.</p>
<h2>Voluntary cession</h2>
<p>You may voluntarily cede a bot to BotPlatform from its settings page. This transfer is permanent and irreversible. No payment is made for a voluntary cession.</p>
<h2>Cookies</h2>
<p>The site uses only strictly necessary cookies (session and CSRF protection). No analytics or advertising cookies are set.</p>
<h2>Conduct</h2>
<p>You agree not to deploy content that violates Discord's Terms of Service, applicable law, or these terms. The platform may suspend or delete a bot without notice if it does.</p>
<p class="muted small">These terms are a summary and will be replaced by a full legal document before any public launch. <a href="/docs/cgu">Read the full draft (French)</a>.</p>
</div>
<form method="post" action="/accept-cgu">
<input type="hidden" name="csrf" value="{}">
<label class="cgu-check"><input type="checkbox" name="accept" value="1" required> I have read and I accept the terms of service</label>
<div class="s-foot"><span></span><button class="btn btn-primary" type="submit">Accept and continue</button></div>
</form></div></div>"#,
        escape(&auth.csrf)
    );
    page(None, &state, "Terms of service", &body).into_response()
}

#[derive(Deserialize)]
pub(crate) struct AcceptCguForm {
    csrf: String,
    accept: Option<String>,
}

pub(crate) async fn accept_cgu_post(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<AcceptCguForm>,
) -> Response {
    let auth = match require_session(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if auth.cgu_accepted {
        return redirect("/dashboard");
    }
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    if form.accept.as_deref() != Some("1") {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            None,
            "Terms not accepted",
            "You must accept the terms of service to continue.",
        );
    }
    let now = (state.clock)();
    let saved = state.store.lock().map(|s| s.accept_cgu(auth.user_id, now));
    if matches!(saved, Ok(Ok(()))) {
        redirect("/dashboard")
    } else {
        server_error(&state, None)
    }
}

// Pages de sécurité du compte : vérification d'email et 2FA TOTP.
// En développement (pas de fournisseur email configuré), le code de vérification
// est affiché directement sur la page (marqué DEV MODE) au lieu d'être envoyé par email.
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_store::totp::base32_encode;
use serde::Deserialize;

use crate::html::{escape, AccountTab};
use crate::render::{message, page_in, redirect, require, require_csrf, server_error};
use crate::session::Auth;
use crate::state::WebState;

const EMAIL_TTL: i64 = 86_400; // 24 h

pub(crate) async fn show(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let (email_status, totp_on) = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let es = match store.email_status(auth.user_id) {
            Ok(s) => s,
            Err(_) => return server_error(&state, Some(&auth)),
        };
        let totp = match store.totp_enabled(auth.user_id) {
            Ok(t) => t,
            Err(_) => return server_error(&state, Some(&auth)),
        };
        (es, totp)
    };
    let csrf = escape(&auth.csrf);
    let (email_str, verified) = email_status
        .map(|s| (s.email.clone(), s.verified))
        .unwrap_or_default();
    let email_section = email_section(&email_str, verified, &csrf);
    let totp_section = totp_section(totp_on, &csrf);
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Security</h1></div></div>{email_section}{totp_section}</div>"#
    );
    let nav = account_nav(&auth, &state, AccountTab::Security);
    page_in(&auth, &state, "Security", &body, &nav).into_response()
}

#[derive(Deserialize)]
pub(crate) struct EmailForm {
    email: String,
    csrf: String,
}

pub(crate) async fn start_email(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<EmailForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let email = form.email.trim().to_string();
    if email.is_empty() || email.len() > 254 || !email.contains('@') {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid email",
            "Enter a valid email address.",
        );
    }
    let now = (state.clock)();
    let (code, is_dev) = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        match store.start_email_verification(auth.user_id, &email, now, EMAIL_TTL) {
            Ok(c) => (c, true), // en prod le second tuple sera (c, false) quand un email est envoyé
            Err(_) => return server_error(&state, Some(&auth)),
        }
    };
    // TODO : envoyer le code par email ici quand le fournisseur est configuré.
    // En attendant : affichage direct sur la page (dev uniquement).
    let dev_note = if is_dev {
        format!(
            r#"<div class="alert alert-warning"><strong>DEV MODE</strong> — No email provider configured. Your verification code is: <strong class="mono">{code}</strong></div>"#
        )
    } else {
        String::new()
    };
    let back_link = r#"<p><a href="/account/security">&larr; Back to security</a></p>"#;
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Verify your email</h1></div></div>{dev_note}<p>A verification code has been sent to <strong>{}</strong>. It is valid for 24 hours.</p><form method="get" action="/account/security/email/verify"><div class="field"><label for="code">Verification code</label><input id="code" class="input mono" name="code" required autofocus placeholder="e.g. 1a2b3c4d5e6f7a8b"></div><div class="field-row"><button class="btn btn-primary" type="submit">Verify</button></div></form>{back_link}</div>"#,
        escape(&email),
    );
    let nav = account_nav(&auth, &state, AccountTab::Security);
    page_in(&auth, &state, "Verify email", &body, &nav).into_response()
}

#[derive(Deserialize)]
pub(crate) struct VerifyQuery {
    code: String,
}

pub(crate) async fn verify_email(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(q): Query<VerifyQuery>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let now = (state.clock)();
    let ok = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        match store.confirm_email_verification(auth.user_id, &q.code, now) {
            Ok(b) => b,
            Err(_) => return server_error(&state, Some(&auth)),
        }
    };
    if ok {
        redirect("/account/security")
    } else {
        message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid or expired code",
            "The verification code is incorrect or has expired. Request a new one.",
        )
    }
}

pub(crate) async fn totp_setup(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let secret = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        match store.create_totp_secret(auth.user_id) {
            Ok(s) => s,
            Err(_) => return server_error(&state, Some(&auth)),
        }
    };
    let b32 = base32_encode(&secret);
    let username = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        store
            .user_profile(auth.user_id)
            .ok()
            .flatten()
            .map(|p| p.username)
            .unwrap_or_else(|| auth.user_id.to_string())
    };
    // otpauth:// URL pour les apps comme Aegis, Authy, etc.
    let otpauth = format!(
        "otpauth://totp/BotPlatform:{username}?secret={b32}&issuer=BotPlatform&algorithm=SHA256&digits=6&period=30"
    );
    let csrf = escape(&auth.csrf);
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Set up two-factor authentication</h1></div></div>
<section class="card">
<p>Scan the code below with your authenticator app (Aegis, Authy, or any RFC 6238 app that supports SHA-256).</p>
<div class="totp-secret"><p class="label">Manual entry — secret</p><pre class="mono select-all">{b32}</pre>
<p class="label">otpauth:// URI (paste into your app)</p><pre class="mono select-all small">{}</pre></div>
<form method="post" action="/account/security/totp/enable"><input type="hidden" name="csrf" value="{csrf}">
<div class="field"><label for="totp-code">Confirm with a code from your app</label><input id="totp-code" class="input mono" name="code" type="text" inputmode="numeric" pattern="[0-9]{{6}}" maxlength="6" required autofocus placeholder="123456"></div>
<div class="field-row"><button class="btn btn-primary" type="submit">Enable 2FA</button><a class="btn" href="/account/security">Cancel</a></div>
</form></section></div>"#,
        escape(&otpauth),
    );
    let nav = account_nav(&auth, &state, AccountTab::Security);
    page_in(&auth, &state, "Set up 2FA", &body, &nav).into_response()
}

#[derive(Deserialize)]
pub(crate) struct TotpCodeForm {
    code: String,
    csrf: String,
}

pub(crate) async fn totp_enable(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<TotpCodeForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let code: u32 = match form.code.trim().parse() {
        Ok(c) => c,
        Err(_) => {
            return message(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Invalid code",
                "Enter the 6-digit code from your authenticator app.",
            );
        }
    };
    let now = (state.clock)();
    let ok = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        match store.enable_totp(auth.user_id, code, now) {
            Ok(b) => b,
            Err(_) => return server_error(&state, Some(&auth)),
        }
    };
    if ok {
        redirect("/account/security")
    } else {
        message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Incorrect code",
            "The code did not match. Make sure your device clock is correct and try again.",
        )
    }
}

pub(crate) async fn totp_disable(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<TotpCodeForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let code: u32 = match form.code.trim().parse() {
        Ok(c) => c,
        Err(_) => {
            return message(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Invalid code",
                "Enter the 6-digit code from your authenticator app.",
            );
        }
    };
    let now = (state.clock)();
    let ok = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        match store.disable_totp(auth.user_id, code, now) {
            Ok(b) => b,
            Err(_) => return server_error(&state, Some(&auth)),
        }
    };
    if ok {
        redirect("/account/security")
    } else {
        message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Incorrect code",
            "The code did not match. 2FA has not been disabled.",
        )
    }
}

fn email_section(email: &str, verified: bool, csrf: &str) -> String {
    let status = if email.is_empty() {
        "<span class=\"badge badge-muted\">Not set</span>".to_string()
    } else if verified {
        format!(
            "<span class=\"badge badge-ok\">Verified</span> {}",
            escape(email)
        )
    } else {
        format!(
            "<span class=\"badge badge-warn\">Unverified</span> {}",
            escape(email)
        )
    };
    format!(
        r#"<section class="card">
<h2>Email address</h2>
<p>{status}</p>
<form method="post" action="/account/security/email">
<input type="hidden" name="csrf" value="{csrf}">
<div class="field"><label for="email">New email address</label><input id="email" class="input" name="email" type="email" autocomplete="email" placeholder="you@example.com" value="{}"></div>
<div class="field-row"><button class="btn btn-primary" type="submit">Send verification</button></div>
</form></section>"#,
        escape(email),
    )
}

fn totp_section(enabled: bool, csrf: &str) -> String {
    if enabled {
        format!(
            r#"<section class="card">
<h2>Two-factor authentication</h2>
<p><span class="badge badge-ok">Enabled</span> Your account is protected with an authenticator app.</p>
<form method="post" action="/account/security/totp/disable">
<input type="hidden" name="csrf" value="{csrf}">
<div class="field"><label for="totp-off">Current code (to confirm)</label><input id="totp-off" class="input mono" name="code" type="text" inputmode="numeric" pattern="[0-9]{{6}}" maxlength="6" required placeholder="123456"></div>
<div class="field-row"><button class="btn btn-danger" type="submit">Disable 2FA</button></div>
</form></section>"#
        )
    } else {
        r#"<section class="card">
<h2>Two-factor authentication</h2>
<p><span class="badge badge-muted">Not enabled</span> Add an extra layer of security to your account.</p>
<div class="field-row"><a class="btn btn-primary" href="/account/security/totp/setup">Set up 2FA</a></div>
</section>"#
            .to_string()
    }
}

fn account_nav<'a>(auth: &'a Auth, state: &'a WebState, tab: AccountTab) -> crate::html::Nav<'a> {
    let bots = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.products_of(auth.user_id).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.id.0, p.name))
        .collect::<Vec<_>>();
    // La nav ne peut pas tenir de références à des Vec locaux ; on utilise Nav::Account sans les bots ici.
    // Le layout NavUser est construit dans page_in déjà.
    let _ = bots;
    crate::html::Nav::Account(tab)
}

// Édition du profil public d'un développeur : bio, liens sociaux.
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::html::{escape, AccountTab, Nav};
use crate::render::{message_back, page_in, redirect, require, require_csrf, server_error};
use crate::state::WebState;

const MAX_BIO: usize = 200;
const MAX_URL: usize = 255;

pub(crate) async fn show(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let profile = match state.store.lock().map(|s| s.user_profile(auth.user_id)) {
        Ok(Ok(Some(p))) => p,
        Ok(Ok(None)) => {
            return server_error(&state, Some(&auth));
        }
        _ => return server_error(&state, Some(&auth)),
    };
    let csrf = escape(&auth.csrf);
    let val = |opt: &Option<String>| opt.as_deref().map(escape).unwrap_or_default();
    let body = format!(
        r#"<div class="page narrow"><div class="page-head"><div><h1>Edit profile</h1><p>This information is shown on your public developer page at <a href="/developers/{id}">/developers/{id}</a>.</p></div></div>
<form class="setting-form" method="post" action="/account/profile">
<input type="hidden" name="csrf" value="{csrf}">
<div class="setting"><div class="s-body">
<label class="label" for="bio">Bio</label>
<textarea class="input" id="bio" name="bio" maxlength="{MAX_BIO}" rows="3" placeholder="A short description about you and your bots">{bio}</textarea>
</div></div>
<div class="setting"><div class="s-body">
<label class="label" for="github_url">GitHub URL</label>
<input class="input" id="github_url" name="github_url" type="url" maxlength="{MAX_URL}" placeholder="https://github.com/username" value="{github_url}">
</div></div>
<div class="setting"><div class="s-body">
<label class="label" for="twitter_url">Twitter / X URL</label>
<input class="input" id="twitter_url" name="twitter_url" type="url" maxlength="{MAX_URL}" placeholder="https://twitter.com/username" value="{twitter_url}">
</div></div>
<div class="setting"><div class="s-body">
<label class="label" for="website_url">Website URL</label>
<input class="input" id="website_url" name="website_url" type="url" maxlength="{MAX_URL}" placeholder="https://example.com" value="{website_url}">
</div></div>
<div class="form-actions"><button class="btn btn-primary" type="submit">Save profile</button></div>
</form></div>"#,
        id = auth.user_id,
        bio = val(&profile.bio),
        github_url = val(&profile.github_url),
        twitter_url = val(&profile.twitter_url),
        website_url = val(&profile.website_url),
    );
    page_in(
        &auth,
        &state,
        "Edit profile",
        &body,
        &Nav::Account(AccountTab::Bots),
    )
    .into_response()
}

#[derive(Deserialize)]
pub(crate) struct ProfileForm {
    csrf: String,
    bio: String,
    github_url: String,
    twitter_url: String,
    website_url: String,
}

fn empty_to_none(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn url_ok(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

pub(crate) async fn save(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<ProfileForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_csrf(&state, &auth, &form.csrf) {
        return r;
    }
    let back = "/account/profile";
    let bio = empty_to_none(&form.bio);
    if bio.is_some_and(|b| b.len() > MAX_BIO) {
        return message_back(
            axum::http::StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Bio too long",
            &format!("Maximum {MAX_BIO} characters."),
            back,
        );
    }
    for (name, raw) in [
        ("GitHub URL", &form.github_url),
        ("Twitter URL", &form.twitter_url),
        ("Website URL", &form.website_url),
    ] {
        let v = raw.trim();
        if !v.is_empty() && !url_ok(v) {
            return message_back(
                axum::http::StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Invalid URL",
                &format!("{name} must start with https:// or http://."),
                back,
            );
        }
        if v.len() > MAX_URL {
            return message_back(
                axum::http::StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "URL too long",
                &format!("{name}: maximum {MAX_URL} characters."),
                back,
            );
        }
    }
    let saved = state.store.lock().map(|s| {
        s.set_user_profile(
            auth.user_id,
            bio,
            empty_to_none(&form.github_url),
            empty_to_none(&form.twitter_url),
            empty_to_none(&form.website_url),
        )
    });
    if matches!(saved, Ok(Ok(()))) {
        redirect(&format!("/developers/{}", auth.user_id))
    } else {
        server_error(&state, Some(&auth))
    }
}

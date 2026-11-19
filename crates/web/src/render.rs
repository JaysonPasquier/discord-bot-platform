use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::html::{escape, layout, Nav, NavUser};
use crate::session::{authenticate, tokens_match, Auth};
use crate::state::WebState;

#[derive(Deserialize)]
pub(crate) struct CsrfForm {
    pub csrf: String,
}

fn nav_user(a: &Auth) -> NavUser<'_> {
    NavUser {
        id: a.user_id,
        name: &a.name,
        csrf: &a.csrf,
        bots: &a.bots,
        subscriptions: a.subscriptions,
    }
}

// Page publique : la navigation du site, le compte (s'il y en a un) à droite.
pub(crate) fn page(auth: Option<&Auth>, state: &WebState, title: &str, body: &str) -> Html<String> {
    let user = auth.map(nav_user);
    let banner = state.config.maintenance_banner.as_deref();
    Html(layout(title, user.as_ref(), &Nav::Public, body, banner))
}

// Page d'application : en-tête avec portée, recherche et onglets (comptes, ou un bot).
pub(crate) fn page_in(
    auth: &Auth,
    state: &WebState,
    title: &str,
    body: &str,
    nav: &Nav<'_>,
) -> Html<String> {
    let user = nav_user(auth);
    let banner = state.config.maintenance_banner.as_deref();
    Html(layout(title, Some(&user), nav, body, banner))
}

pub(crate) fn message(
    status: StatusCode,
    state: &WebState,
    auth: Option<&Auth>,
    title: &str,
    text: &str,
) -> Response {
    message_back(status, state, auth, title, text, "/")
}

// `back` est toujours un chemin interne choisi par le code, jamais une valeur reçue.
pub(crate) fn message_back(
    status: StatusCode,
    state: &WebState,
    auth: Option<&Auth>,
    title: &str,
    text: &str,
    back: &str,
) -> Response {
    let body = format!(
        r#"<h1>{}</h1><p>{}</p><p><a href="{}">Back</a></p>"#,
        escape(title),
        escape(text),
        escape(back)
    );
    (status, page(auth, state, title, &body)).into_response()
}

pub(crate) fn redirect(to: &str) -> Response {
    Redirect::to(to).into_response()
}

pub(crate) fn with_cookies(mut response: Response, cookies: &[String]) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(SET_COOKIE, value);
        }
    }
    response
}

// Exige une session valide ET l'acceptation des CGU (toutes les pages authentifiées sauf /accept-cgu).
pub(crate) fn require(state: &WebState, headers: &HeaderMap) -> Result<Auth, Response> {
    let auth = authenticate(state, headers).ok_or_else(|| redirect("/login"))?;
    if auth.cgu_accepted {
        Ok(auth)
    } else {
        Err(redirect("/accept-cgu"))
    }
}

// Exige uniquement une session valide, sans vérifier les CGU (pour /accept-cgu lui-même).
pub(crate) fn require_session(state: &WebState, headers: &HeaderMap) -> Result<Auth, Response> {
    authenticate(state, headers).ok_or_else(|| redirect("/login"))
}

pub(crate) fn require_csrf(state: &WebState, auth: &Auth, token: &str) -> Result<(), Response> {
    if tokens_match(&auth.csrf, token) {
        Ok(())
    } else {
        Err(message(
            StatusCode::FORBIDDEN,
            state,
            Some(auth),
            "Forbidden",
            "Invalid form token. Reload the page and try again.",
        ))
    }
}

pub(crate) fn not_found(state: &WebState, auth: &Auth) -> Response {
    message(
        StatusCode::NOT_FOUND,
        state,
        Some(auth),
        "Not found",
        "This page does not exist.",
    )
}

pub(crate) fn server_error(state: &WebState, auth: Option<&Auth>) -> Response {
    message(
        StatusCode::INTERNAL_SERVER_ERROR,
        state,
        auth,
        "Something went wrong",
        "The platform could not complete this action.",
    )
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;
    use crate::state::test_state;

    async fn text(response: Response) -> (StatusCode, String) {
        let status = response.status();
        let Ok(bytes) = to_bytes(response.into_body(), 1 << 16).await else {
            panic!("cannot read body");
        };
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn auth() -> Auth {
        Auth {
            user_id: 1,
            name: "n".to_string(),
            csrf: "secret".to_string(),
            guilds: vec![],
            cgu_accepted: true,
            bots: vec![],
            subscriptions: 0,
        }
    }

    #[tokio::test]
    async fn a_server_error_is_a_500_with_a_fixed_message() {
        let (status, body) = text(server_error(&test_state(), None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body.contains("The platform could not complete this action."));
    }

    #[tokio::test]
    async fn a_missing_page_is_a_404() {
        let (status, body) = text(not_found(&test_state(), &auth())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("This page does not exist."));
    }

    #[tokio::test]
    async fn messages_escape_their_title_and_text() {
        let response = message(
            StatusCode::BAD_REQUEST,
            &test_state(),
            None,
            "<i>t</i>",
            "a & b",
        );
        let (status, body) = text(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("&lt;i&gt;t&lt;/i&gt;") && body.contains("a &amp; b"));
        assert!(!body.contains("<i>t</i>"));
    }

    #[tokio::test]
    async fn the_back_link_defaults_to_home_and_can_point_elsewhere() {
        let state = test_state();
        let (_, home) = text(message(StatusCode::OK, &state, None, "t", "x")).await;
        assert!(home.contains(r#"<a href="/">Back</a>"#));
        let (_, other) = text(message_back(
            StatusCode::OK,
            &state,
            None,
            "t",
            "x",
            "/products/7",
        ))
        .await;
        assert!(other.contains(r#"<a href="/products/7">Back</a>"#));
    }

    #[tokio::test]
    async fn only_the_matching_form_token_passes() {
        let state = test_state();
        assert!(require_csrf(&state, &auth(), "secret").is_ok());
        let Err(response) = require_csrf(&state, &auth(), "wrong") else {
            panic!("a wrong token was accepted");
        };
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(require_csrf(&state, &auth(), "").is_err());
    }

    #[test]
    fn redirects_go_where_asked_and_cookies_are_appended() {
        let response = with_cookies(redirect("/x"), &["a=1".to_string(), "b=2".to_string()]);
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok());
        assert_eq!(location, Some("/x"));
        assert_eq!(response.headers().get_all(SET_COOKIE).iter().count(), 2);
        let invalid = with_cookies(redirect("/x"), &["bad\nvalue".to_string()]);
        assert_eq!(invalid.headers().get_all(SET_COOKIE).iter().count(), 0);
    }
}

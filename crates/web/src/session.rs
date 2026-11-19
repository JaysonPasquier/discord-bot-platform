use axum::http::header::COOKIE;
use axum::http::HeaderMap;
use platform_store::{generate_token, hash_token};

use crate::discord::{DiscordUser, Guild};
use crate::state::WebState;

pub const SESSION_COOKIE: &str = "session";
pub const STATE_COOKIE: &str = "oauth_state";
pub const SESSION_TTL_SECS: u32 = 7 * 24 * 3600;
pub const STATE_TTL_SECS: u32 = 600;

pub struct Auth {
    pub user_id: u64,
    pub name: String,
    pub csrf: String,
    pub guilds: Vec<Guild>,
    pub cgu_accepted: bool,
    // Pour l'en-tête et la palette de commandes : les bots de l'utilisateur et le nombre de ses abonnements.
    pub bots: Vec<(u64, String)>,
    pub subscriptions: usize,
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then(|| value.to_string())
        })
}

pub fn set_cookie(name: &str, value: &str, max_age_secs: u32, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}")
}

pub fn clear_cookie(name: &str, secure: bool) -> String {
    set_cookie(name, "", 0, secure)
}

// Comparaison en temps constant : un jeton CSRF ne se devine pas octet par octet.
pub fn tokens_match(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn authenticate(state: &WebState, headers: &HeaderMap) -> Option<Auth> {
    let cookie = cookie_value(headers, SESSION_COOKIE)?;
    let now = (state.clock)();
    let store = state.store.lock().ok()?;
    let session = store.session(&hash_token(&cookie), now).ok()??;
    let bots = store
        .products_of(session.discord_id)
        .map(|mine| mine.into_iter().map(|p| (p.id.0, p.name)).collect())
        .unwrap_or_default();
    let subscriptions = store
        .subscriptions_of(session.discord_id)
        .map_or(0, |subs| subs.len());
    Some(Auth {
        user_id: session.discord_id,
        name: session.username,
        csrf: session.csrf,
        guilds: serde_json::from_str(&session.guilds).ok()?,
        cgu_accepted: session.cgu_accepted,
        bots,
        subscriptions,
    })
}

// Toujours une nouvelle session à la connexion : pas de fixation de session.
pub fn start_session(state: &WebState, user: &DiscordUser, guilds: &[Guild]) -> Option<String> {
    let cookie = generate_token().ok()?;
    let csrf = generate_token().ok()?;
    let guilds_json = serde_json::to_string(guilds).ok()?;
    let now = (state.clock)();
    let expires_at = now.saturating_add(i64::from(SESSION_TTL_SECS));
    let store = state.store.lock().ok()?;
    store.upsert_user(user.id, &user.name).ok()?;
    store
        .create_session(
            &hash_token(&cookie),
            user.id,
            &csrf,
            &guilds_json,
            expires_at,
            now,
        )
        .ok()?;
    Some(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for v in values {
            map.append(
                COOKIE,
                HeaderValue::from_str(v).unwrap_or(HeaderValue::from_static("")),
            );
        }
        map
    }

    #[test]
    fn a_cookie_is_found_among_several() {
        let h = headers(&["a=1; session=abc; b=2"]);
        assert_eq!(cookie_value(&h, "session").as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "a").as_deref(), Some("1"));
        assert_eq!(cookie_value(&h, "b").as_deref(), Some("2"));
        assert_eq!(cookie_value(&h, "c"), None);
    }

    #[test]
    fn cookies_may_come_in_several_headers() {
        let h = headers(&["a=1", "session=xyz"]);
        assert_eq!(cookie_value(&h, "session").as_deref(), Some("xyz"));
    }

    #[test]
    fn names_must_match_exactly() {
        let h = headers(&["xsession=bad; session2=bad; session=good"]);
        assert_eq!(cookie_value(&h, "session").as_deref(), Some("good"));
        assert_eq!(cookie_value(&headers(&["xsession=bad"]), "session"), None);
    }

    #[test]
    fn values_may_contain_equals_signs() {
        assert_eq!(
            cookie_value(&headers(&["session=a=b"]), "session").as_deref(),
            Some("a=b")
        );
        assert_eq!(cookie_value(&headers(&["novalue"]), "novalue"), None);
        assert_eq!(cookie_value(&HeaderMap::new(), "session"), None);
    }

    #[test]
    fn session_cookies_are_httponly_lax_and_secure_over_https() {
        assert_eq!(
            set_cookie("session", "abc", 60, false),
            "session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=60"
        );
        assert_eq!(
            set_cookie("session", "abc", 60, true),
            "session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=60; Secure"
        );
    }

    #[test]
    fn clearing_a_cookie_expires_it_immediately() {
        assert_eq!(
            clear_cookie("session", false),
            "session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
        );
        assert!(clear_cookie("session", true).ends_with("; Secure"));
    }

    #[test]
    fn tokens_are_compared_whole() {
        assert!(tokens_match("abc", "abc"));
        assert!(tokens_match("", ""));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "abcd"));
        assert!(!tokens_match("abc", "ab"));
        assert!(!tokens_match("a", ""));
        assert!(!tokens_match("ab", "ba"));
        assert!(!tokens_match("abc", "cba"));
    }
}

use std::time::Duration;

use crate::base64::base64;
use crate::discord::{
    authorize_url, parse_guilds, parse_token, parse_user, Discord, DiscordError, DiscordUser, Guild,
};
use crate::state::Secret;

pub(crate) const API: &str = "https://discord.com/api/v10";
pub(crate) const USER_AGENT: &str = "DiscordBot (https://github.com/your-org/bot-platform, 0.1.0)";

pub struct HttpDiscord {
    api_base: String,
    client_id: String,
    client_secret: Secret,
    redirect_uri: String,
    agent: ureq::Agent,
}

impl HttpDiscord {
    pub fn new(client_id: String, client_secret: Secret, redirect_uri: String) -> Self {
        Self::with_base(API.to_string(), client_id, client_secret, redirect_uri)
    }

    // L'adresse de l'API est un paramètre pour tester la forme exacte des requêtes contre un faux serveur local.
    pub fn with_base(
        api_base: String,
        client_id: String,
        client_secret: Secret,
        redirect_uri: String,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Self {
            api_base,
            client_id,
            client_secret,
            redirect_uri,
            agent,
        }
    }

    fn basic_auth(&self) -> String {
        let pair = format!("{}:{}", self.client_id, self.client_secret.expose());
        format!("Basic {}", base64(pair.as_bytes()))
    }

    fn get(&self, path: &str, token: &str) -> Result<String, DiscordError> {
        let mut response = self
            .agent
            .get(format!("{}{path}", self.api_base))
            .header("User-Agent", USER_AGENT)
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .map_err(|e| DiscordError::Http(e.to_string()))?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|e| DiscordError::Http(e.to_string()))
    }

    fn exchange(&self, code: &str) -> Result<String, DiscordError> {
        let mut response = self
            .agent
            .post(format!("{}/oauth2/token", self.api_base))
            .header("User-Agent", USER_AGENT)
            .header("Authorization", self.basic_auth())
            .send_form([
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .map_err(|e| DiscordError::Http(e.to_string()))?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| DiscordError::Http(e.to_string()))?;
        parse_token(&body)
    }
}

impl Discord for HttpDiscord {
    fn authorize_url(&self, state: &str) -> Option<String> {
        Some(authorize_url(&self.client_id, &self.redirect_uri, state))
    }

    fn login(&self, code: &str) -> Result<(DiscordUser, Vec<Guild>), DiscordError> {
        let token = self.exchange(code)?;
        let user = parse_user(&self.get("/users/@me", &token)?)?;
        let guilds = parse_guilds(&self.get("/users/@me/guilds", &token)?)?;
        Ok((user, guilds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_testkit::fake_discord;

    #[test]
    fn the_authorization_header_is_basic_with_id_and_secret() {
        let d = HttpDiscord::new(
            "id".to_string(),
            Secret::new("secret"),
            "http://x/cb".to_string(),
        );
        assert_eq!(d.basic_auth(), format!("Basic {}", base64(b"id:secret")));
        assert_eq!(d.basic_auth(), "Basic aWQ6c2VjcmV0");
    }

    const TOKEN_JSON: &str = r#"{"access_token":"tok123","token_type":"Bearer","expires_in":604800,"refresh_token":"r","scope":"identify guilds"}"#;
    const USER_JSON: &str =
        r#"{"id":"80351110224678912","username":"nelly","global_name":"Nelly"}"#;
    const GUILDS_JSON: &str = r#"[{"id":"1","name":"Mine","owner":true,"permissions":"0"},{"id":"2","name":"Other","owner":false,"permissions":"1"}]"#;

    fn client(base: String) -> HttpDiscord {
        HttpDiscord::with_base(
            base,
            "cid".to_string(),
            Secret::new("sec"),
            "http://127.0.0.1:8080/auth/callback".to_string(),
        )
    }

    #[test]
    fn a_full_login_sends_the_documented_requests() {
        let (base, server) = fake_discord(vec![
            (200, TOKEN_JSON),
            (200, USER_JSON),
            (200, GUILDS_JSON),
        ]);
        let result = client(base).login("thecode");
        let seen = server.join().unwrap_or_default();
        let login = result.map(|(u, g)| (u.id, u.name, g.len(), g.first().map(|x| x.name.clone())));
        assert_eq!(
            login,
            Ok((
                80351110224678912,
                "Nelly".to_string(),
                1,
                Some("Mine".to_string())
            ))
        );
        assert_eq!(seen.len(), 3);

        let exchange = seen[0].to_lowercase();
        assert!(exchange.starts_with("post /oauth2/token "), "{exchange}");
        assert!(seen[0].contains("Basic Y2lkOnNlYw=="), "{}", seen[0]);
        assert!(exchange.contains("content-type: application/x-www-form-urlencoded"));
        assert!(exchange.contains("grant_type=authorization_code"));
        assert!(exchange.contains("code=thecode"));
        assert!(
            exchange.contains("redirect_uri=http%3a%2f%2f127.0.0.1%3a8080%2fauth%2fcallback"),
            "{exchange}"
        );
        assert!(exchange
            .contains("user-agent: discordbot (https://github.com/your-org/bot-platform, 0.1.0)"));

        let user = seen[1].to_lowercase();
        assert!(user.starts_with("get /users/@me "), "{user}");
        assert!(user.contains("authorization: bearer tok123"));
        assert!(user.contains("user-agent: discordbot ("));
        let guilds = seen[2].to_lowercase();
        assert!(guilds.starts_with("get /users/@me/guilds "), "{guilds}");
        assert!(guilds.contains("authorization: bearer tok123"));
    }

    #[test]
    fn a_refused_exchange_stops_the_login_after_one_request() {
        let (base, server) = fake_discord(vec![(400, r#"{"error":"invalid_grant"}"#)]);
        assert!(matches!(
            client(base).login("x"),
            Err(DiscordError::Http(_))
        ));
        assert_eq!(server.join().map(|s| s.len()).ok(), Some(1));
    }

    #[test]
    fn a_failure_on_the_user_or_guild_request_fails_the_login() {
        let (base, server) = fake_discord(vec![(200, TOKEN_JSON), (401, "{}")]);
        assert!(matches!(
            client(base).login("x"),
            Err(DiscordError::Http(_))
        ));
        assert_eq!(server.join().map(|s| s.len()).ok(), Some(2));
        let (base, _) = fake_discord(vec![(200, TOKEN_JSON), (200, USER_JSON), (500, "{}")]);
        assert!(matches!(
            client(base).login("x"),
            Err(DiscordError::Http(_))
        ));
    }

    #[test]
    fn unreadable_answers_are_parse_errors() {
        let (base, _) = fake_discord(vec![(200, "{}")]);
        assert_eq!(client(base).login("x").err(), Some(DiscordError::Parse));
        let (base, _) = fake_discord(vec![(200, TOKEN_JSON), (200, "not json")]);
        assert_eq!(client(base).login("x").err(), Some(DiscordError::Parse));
        let (base, _) = fake_discord(vec![(200, TOKEN_JSON), (200, USER_JSON), (200, "{}")]);
        assert_eq!(client(base).login("x").err(), Some(DiscordError::Parse));
    }

    #[test]
    fn an_unreachable_server_is_an_http_error() {
        let result = client("http://127.0.0.1:1".to_string()).login("x");
        assert!(matches!(result, Err(DiscordError::Http(_))));
    }

    #[test]
    fn the_real_client_builds_a_login_url() {
        let d = HttpDiscord::new(
            "123".to_string(),
            Secret::new("s"),
            "http://127.0.0.1:8080/auth/callback".to_string(),
        );
        let url = d.authorize_url("abc").unwrap_or_default();
        assert!(url
            .starts_with("https://discord.com/oauth2/authorize?response_type=code&client_id=123&"));
        assert!(url.contains("state=abc"));
    }
}

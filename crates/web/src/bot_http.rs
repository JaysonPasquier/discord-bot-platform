use std::time::Duration;

use crate::bot_app::{parse_application, parse_guild_count, BotApp, BotVerifier};
use crate::discord::DiscordError;
use crate::discord_http::{API, USER_AGENT};

// Demande à Discord quelle application possède ce jeton de bot : `GET /applications/@me`.
pub struct HttpBotVerifier {
    api_base: String,
    agent: ureq::Agent,
}

impl HttpBotVerifier {
    pub fn new() -> Self {
        Self::with_base(API.to_string())
    }

    pub fn with_base(api_base: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            // Un jeton refusé doit rester une réponse 401, pas une erreur de transport.
            .http_status_as_error(false)
            .build()
            .into();
        Self { api_base, agent }
    }
}

impl Default for HttpBotVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpBotVerifier {
    fn application(&self, token: &str) -> Result<String, DiscordError> {
        let mut response = self
            .agent
            .get(format!("{}/applications/@me", self.api_base))
            .header("User-Agent", USER_AGENT)
            .header("Authorization", format!("Bot {token}"))
            .call()
            .map_err(|e| DiscordError::Http(e.to_string()))?;
        match response.status().as_u16() {
            200 => response
                .body_mut()
                .read_to_string()
                .map_err(|e| DiscordError::Http(e.to_string())),
            401 | 403 => Err(DiscordError::Rejected),
            other => Err(DiscordError::Http(format!("unexpected status {other}"))),
        }
    }
}

impl BotVerifier for HttpBotVerifier {
    fn verify(&self, token: &str) -> Result<BotApp, DiscordError> {
        parse_application(&self.application(token)?)
    }

    fn guild_count(&self, token: &str) -> Option<u64> {
        parse_guild_count(&self.application(token).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_testkit::fake_discord;

    const APP_JSON: &str = r#"{"id":"1234567895647001626","name":"Alice Bot"}"#;

    #[test]
    fn the_request_is_the_documented_one_with_the_bot_prefix() {
        let (base, server) = fake_discord(vec![(200, APP_JSON)]);
        let app = HttpBotVerifier::with_base(base).verify("the.bot.token");
        let seen = server.join().unwrap_or_default();
        assert_eq!(
            app,
            Ok(BotApp {
                id: "1234567895647001626".to_string(),
                name: "Alice Bot".to_string()
            })
        );
        assert_eq!(seen.len(), 1);
        let request = seen[0].to_lowercase();
        assert!(request.starts_with("get /applications/@me "), "{request}");
        assert!(
            request.contains("authorization: bot the.bot.token"),
            "{request}"
        );
        assert!(request.contains("user-agent: discordbot ("), "{request}");
    }

    #[test]
    fn the_guild_count_comes_from_the_same_documented_request() {
        let json =
            r#"{"id":"1234567895647001626","name":"Alice Bot","approximate_guild_count":17}"#;
        let (base, server) = fake_discord(vec![(200, json)]);
        assert_eq!(HttpBotVerifier::with_base(base).guild_count("t"), Some(17));
        let seen = server.join().unwrap_or_default();
        assert!(seen[0].to_lowercase().starts_with("get /applications/@me "));
        let (base, _) = fake_discord(vec![(200, APP_JSON)]);
        assert_eq!(HttpBotVerifier::with_base(base).guild_count("t"), None);
        let (base, _) = fake_discord(vec![(401, "{}")]);
        assert_eq!(HttpBotVerifier::with_base(base).guild_count("t"), None);
    }

    #[test]
    fn a_refused_token_is_reported_as_rejected() {
        for status in [401, 403] {
            let (base, _) = fake_discord(vec![(status, r#"{"message":"401: Unauthorized"}"#)]);
            assert_eq!(
                HttpBotVerifier::with_base(base).verify("bad"),
                Err(DiscordError::Rejected),
                "{status}"
            );
        }
    }

    #[test]
    fn other_statuses_and_garbage_are_not_a_rejection() {
        let (base, _) = fake_discord(vec![(500, "{}")]);
        assert!(matches!(
            HttpBotVerifier::with_base(base).verify("t"),
            Err(DiscordError::Http(_))
        ));
        let (base, _) = fake_discord(vec![(429, "{}")]);
        assert!(matches!(
            HttpBotVerifier::with_base(base).verify("t"),
            Err(DiscordError::Http(_))
        ));
        let (base, _) = fake_discord(vec![(200, "not json")]);
        assert_eq!(
            HttpBotVerifier::with_base(base).verify("t"),
            Err(DiscordError::Parse)
        );
    }

    #[test]
    fn an_unreachable_server_is_an_http_error_not_a_rejection() {
        let result = HttpBotVerifier::with_base("http://127.0.0.1:1".to_string()).verify("t");
        assert!(matches!(result, Err(DiscordError::Http(_))));
    }
}

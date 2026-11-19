use std::time::Duration;

use serde_json::{Map, Value};

use crate::discord_http::{API, USER_AGENT};

// Pour chaque champ : `None` = ne pas toucher, `Some(None)` = revenir au profil par défaut,
// `Some(Some(v))` = nouvelle valeur. Les images sont des « data URI ».
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfilePatch {
    pub nick: Option<Option<String>>,
    pub bio: Option<Option<String>>,
    pub avatar: Option<Option<String>>,
    pub banner: Option<Option<String>>,
}

impl ProfilePatch {
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        for (name, field) in [
            ("nick", &self.nick),
            ("bio", &self.bio),
            ("avatar", &self.avatar),
            ("banner", &self.banner),
        ] {
            if let Some(value) = field {
                map.insert(
                    name.to_string(),
                    value
                        .as_ref()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                );
            }
        }
        Value::Object(map).to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    BadToken,
    NotInServer,
    Forbidden,
    Invalid,
    RateLimited,
    Unavailable,
}

pub trait GuildProfiles: Send + Sync + 'static {
    // Modifie le profil du bot dans un serveur, avec le jeton du bot lui-même.
    fn apply(&self, token: &str, guild: u64, patch: &ProfilePatch) -> Result<(), ProfileError>;
}

// `PATCH /guilds/{guild.id}/members/@me` : pseudo, avatar, bannière et bio propres au serveur.
pub struct HttpGuildProfiles {
    api_base: String,
    agent: ureq::Agent,
}

impl HttpGuildProfiles {
    pub fn new() -> Self {
        Self::with_base(API.to_string())
    }

    pub fn with_base(api_base: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { api_base, agent }
    }
}

impl Default for HttpGuildProfiles {
    fn default() -> Self {
        Self::new()
    }
}

fn error_of(status: u16) -> Option<ProfileError> {
    match status {
        200..=299 => None,
        400 => Some(ProfileError::Invalid),
        401 => Some(ProfileError::BadToken),
        403 => Some(ProfileError::Forbidden),
        404 => Some(ProfileError::NotInServer),
        429 => Some(ProfileError::RateLimited),
        _ => Some(ProfileError::Unavailable),
    }
}

impl GuildProfiles for HttpGuildProfiles {
    fn apply(&self, token: &str, guild: u64, patch: &ProfilePatch) -> Result<(), ProfileError> {
        let response = self
            .agent
            .patch(format!("{}/guilds/{guild}/members/@me", self.api_base))
            .header("User-Agent", USER_AGENT)
            .header("Authorization", format!("Bot {token}"))
            .header("Content-Type", "application/json")
            .send(patch.to_json())
            .map_err(|_| ProfileError::Unavailable)?;
        error_of(response.status().as_u16()).map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_testkit::fake_discord;

    fn text(s: &str) -> Option<Option<String>> {
        Some(Some(s.to_string()))
    }

    #[test]
    fn untouched_fields_are_left_out_and_resets_are_null() {
        assert_eq!(ProfilePatch::default().to_json(), "{}");
        let patch = ProfilePatch {
            nick: text("Rex"),
            bio: Some(None),
            avatar: None,
            banner: text("data:image/png;base64,AAAA"),
        };
        let Ok(json) = serde_json::from_str::<Value>(&patch.to_json()) else {
            panic!("not json");
        };
        assert_eq!(json["nick"], "Rex");
        assert_eq!(json["bio"], Value::Null);
        assert!(json.get("bio").is_some());
        assert!(json.get("avatar").is_none());
        assert_eq!(json["banner"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn special_characters_are_escaped_in_the_json() {
        let patch = ProfilePatch {
            nick: text("a\"b\\c\nd"),
            ..ProfilePatch::default()
        };
        let Ok(json) = serde_json::from_str::<Value>(&patch.to_json()) else {
            panic!("not json");
        };
        assert_eq!(json["nick"], "a\"b\\c\nd");
    }

    #[test]
    fn statuses_map_to_the_matching_error() {
        assert_eq!(error_of(200), None);
        assert_eq!(error_of(204), None);
        assert_eq!(error_of(299), None);
        assert_eq!(error_of(400), Some(ProfileError::Invalid));
        assert_eq!(error_of(401), Some(ProfileError::BadToken));
        assert_eq!(error_of(403), Some(ProfileError::Forbidden));
        assert_eq!(error_of(404), Some(ProfileError::NotInServer));
        assert_eq!(error_of(429), Some(ProfileError::RateLimited));
        assert_eq!(error_of(500), Some(ProfileError::Unavailable));
        assert_eq!(error_of(300), Some(ProfileError::Unavailable));
        assert_eq!(error_of(199), Some(ProfileError::Unavailable));
    }

    #[test]
    fn the_request_is_a_patch_of_the_current_member_with_the_bot_token() {
        let (base, server) = fake_discord(vec![(200, "{}")]);
        let patch = ProfilePatch {
            nick: text("Rex"),
            ..ProfilePatch::default()
        };
        let result = HttpGuildProfiles::with_base(base).apply("bot.tok.en", 123456789, &patch);
        let seen = server.join().unwrap_or_default();
        assert_eq!(result, Ok(()));
        assert_eq!(seen.len(), 1);
        let request = seen[0].to_lowercase();
        assert!(
            request.starts_with("patch /guilds/123456789/members/@me "),
            "{request}"
        );
        assert!(
            request.contains("authorization: bot bot.tok.en"),
            "{request}"
        );
        assert!(
            request.contains("content-type: application/json"),
            "{request}"
        );
        assert!(request.contains("user-agent: discordbot ("), "{request}");
        assert!(seen[0].ends_with(r#"{"nick":"Rex"}"#), "{}", seen[0]);
    }

    #[test]
    fn a_refusal_from_discord_is_an_error_and_an_unreachable_server_too() {
        let (base, _) = fake_discord(vec![(404, r#"{"message":"Unknown Guild"}"#)]);
        assert_eq!(
            HttpGuildProfiles::with_base(base).apply("t", 1, &ProfilePatch::default()),
            Err(ProfileError::NotInServer)
        );
        let (base, _) = fake_discord(vec![(429, "{}")]);
        assert_eq!(
            HttpGuildProfiles::with_base(base).apply("t", 1, &ProfilePatch::default()),
            Err(ProfileError::RateLimited)
        );
        assert_eq!(
            HttpGuildProfiles::with_base("http://127.0.0.1:1".to_string()).apply(
                "t",
                1,
                &ProfilePatch::default()
            ),
            Err(ProfileError::Unavailable)
        );
    }
}

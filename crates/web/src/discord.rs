use serde::{Deserialize, Serialize};

const ADMINISTRATOR: u64 = 0x8;
const MANAGE_GUILD: u64 = 0x20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordUser {
    pub id: u64,
    pub name: String,
}

// Seuls les serveurs que l'utilisateur peut gérer sont conservés dans la session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscordError {
    Http(String),
    Parse,
    Rejected,
}

pub trait Discord: Send + Sync + 'static {
    fn authorize_url(&self, state: &str) -> Option<String>;
    fn login(&self, code: &str) -> Result<(DiscordUser, Vec<Guild>), DiscordError>;
}

// Pas de connexion Discord configurée : la page de connexion ne propose alors que ce qui existe.
pub struct NoDiscord;

impl Discord for NoDiscord {
    fn authorize_url(&self, _state: &str) -> Option<String> {
        None
    }

    fn login(&self, _code: &str) -> Result<(DiscordUser, Vec<Guild>), DiscordError> {
        Err(DiscordError::Http(
            "discord login is not configured".to_string(),
        ))
    }
}

pub fn can_manage(owner: bool, permissions: &str) -> bool {
    owner
        || (permissions.bytes().all(|b| b.is_ascii_digit())
            && permissions
                .parse::<u64>()
                .is_ok_and(|p| p & ADMINISTRATOR != 0 || p & MANAGE_GUILD != 0))
}

#[derive(Deserialize)]
struct RawUser {
    id: String,
    username: String,
    global_name: Option<String>,
}

#[derive(Deserialize)]
struct RawGuild {
    id: String,
    name: String,
    #[serde(default)]
    owner: bool,
    #[serde(default)]
    permissions: String,
}

#[derive(Deserialize)]
struct RawToken {
    access_token: String,
}

// Un identifiant Discord (« snowflake ») : chiffres ASCII uniquement, jamais plus de 20
// (tient dans un u64). Seule définition de la règle : tout le crate l'utilise.
pub(crate) fn is_snowflake(raw: &str) -> bool {
    !raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn decimal_id(raw: &str) -> Option<u64> {
    if !is_snowflake(raw) {
        return None;
    }
    raw.parse().ok()
}

pub fn parse_token(json: &str) -> Result<String, DiscordError> {
    serde_json::from_str::<RawToken>(json)
        .map(|t| t.access_token)
        .map_err(|_| DiscordError::Parse)
}

pub fn parse_user(json: &str) -> Result<DiscordUser, DiscordError> {
    let raw: RawUser = serde_json::from_str(json).map_err(|_| DiscordError::Parse)?;
    let id = decimal_id(&raw.id).ok_or(DiscordError::Parse)?;
    let name = raw
        .global_name
        .filter(|n| !n.is_empty())
        .unwrap_or(raw.username);
    Ok(DiscordUser { id, name })
}

pub fn parse_guilds(json: &str) -> Result<Vec<Guild>, DiscordError> {
    let raw: Vec<RawGuild> = serde_json::from_str(json).map_err(|_| DiscordError::Parse)?;
    let mut guilds = Vec::new();
    for guild in raw {
        if decimal_id(&guild.id).is_none() {
            return Err(DiscordError::Parse);
        }
        if can_manage(guild.owner, &guild.permissions) {
            guilds.push(Guild {
                id: guild.id,
                name: guild.name,
            });
        }
    }
    Ok(guilds)
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&scope=identify%20guilds&state={}&redirect_uri={}&prompt=none",
        percent_encode(client_id),
        percent_encode(state),
        percent_encode(redirect_uri)
    )
}

pub fn invite_url(client_id: &str) -> String {
    format!(
        "https://discord.com/oauth2/authorize?client_id={}&scope=bot%20applications.commands",
        percent_encode(client_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_configuration_there_is_nothing_to_log_in_to() {
        assert_eq!(NoDiscord.authorize_url("s"), None);
        assert!(matches!(NoDiscord.login("c"), Err(DiscordError::Http(_))));
    }

    #[test]
    fn owners_and_holders_of_manage_guild_or_administrator_can_manage() {
        assert!(can_manage(true, "0"));
        assert!(can_manage(false, "32"));
        assert!(can_manage(false, "8"));
        assert!(can_manage(false, "40"));
        assert!(can_manage(false, "18446744073709551615"));
        assert!(!can_manage(false, "0"));
        assert!(!can_manage(false, "1"));
        assert!(!can_manage(false, "16"));
        assert!(!can_manage(false, "36953089"));
    }

    #[test]
    fn malformed_permissions_never_grant_access() {
        for bad in [
            "",
            "abc",
            "-32",
            "+32",
            "1e2",
            "32 ",
            "18446744073709551616",
        ] {
            assert!(!can_manage(false, bad), "{bad:?}");
        }
    }

    #[test]
    fn the_token_response_gives_the_access_token() {
        let json = r#"{"access_token":"abc","token_type":"Bearer","expires_in":604800,"refresh_token":"r","scope":"identify"}"#;
        assert_eq!(parse_token(json), Ok("abc".to_string()));
        assert_eq!(parse_token("{}"), Err(DiscordError::Parse));
        assert_eq!(parse_token("nope"), Err(DiscordError::Parse));
    }

    #[test]
    fn the_display_name_wins_over_the_username() {
        let both = r#"{"id":"80351110224678912","username":"nelly","global_name":"Nelly"}"#;
        assert_eq!(
            parse_user(both),
            Ok(DiscordUser {
                id: 80351110224678912,
                name: "Nelly".to_string()
            })
        );
        let none = r#"{"id":"1","username":"nelly","global_name":null}"#;
        assert_eq!(parse_user(none).map(|u| u.name), Ok("nelly".to_string()));
        let empty = r#"{"id":"1","username":"nelly","global_name":""}"#;
        assert_eq!(parse_user(empty).map(|u| u.name), Ok("nelly".to_string()));
        let absent = r#"{"id":"1","username":"nelly"}"#;
        assert_eq!(parse_user(absent).map(|u| u.name), Ok("nelly".to_string()));
    }

    #[test]
    fn user_ids_must_be_decimal_strings() {
        for bad in [
            r#"{"id":"abc","username":"x"}"#,
            r#"{"id":"","username":"x"}"#,
            r#"{"id":"+1","username":"x"}"#,
            r#"{"id":5,"username":"x"}"#,
        ] {
            assert_eq!(parse_user(bad), Err(DiscordError::Parse), "{bad}");
        }
    }

    #[test]
    fn only_manageable_guilds_are_kept() {
        let json = r#"[
          {"id":"1","name":"Mine","owner":true,"permissions":"0"},
          {"id":"2","name":"Admin","owner":false,"permissions":"8"},
          {"id":"3","name":"Member","owner":false,"permissions":"1"},
          {"id":"4","name":"Manager","permissions":"32"},
          {"id":"5","name":"Bare"}
        ]"#;
        let names: Vec<String> = parse_guilds(json)
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, ["Mine", "Admin", "Manager"]);
    }

    #[test]
    fn a_bad_guild_id_fails_the_whole_list() {
        assert_eq!(
            parse_guilds(r#"[{"id":"x","name":"n","owner":true}]"#),
            Err(DiscordError::Parse)
        );
        assert_eq!(parse_guilds("{}"), Err(DiscordError::Parse));
        assert_eq!(parse_guilds("[]"), Ok(vec![]));
    }

    #[test]
    fn the_authorization_url_follows_the_documented_shape() {
        assert_eq!(
            authorize_url("123", "http://127.0.0.1:8080/auth/callback", "st"),
            "https://discord.com/oauth2/authorize?response_type=code&client_id=123&scope=identify%20guilds&state=st&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fauth%2Fcallback&prompt=none"
        );
    }

    #[test]
    fn parameters_are_percent_encoded() {
        let url = authorize_url("1&x=2", "http://h/?a=b c", "s t");
        assert!(url.contains("client_id=1%26x%3D2&"), "{url}");
        assert!(url.contains("state=s%20t&"), "{url}");
        assert!(
            url.contains("redirect_uri=http%3A%2F%2Fh%2F%3Fa%3Db%20c&"),
            "{url}"
        );
        assert_eq!(percent_encode("a-b.c_d~e"), "a-b.c_d~e");
    }

    #[test]
    fn the_bot_invite_url_uses_the_bot_scope() {
        assert_eq!(
            invite_url("42"),
            "https://discord.com/oauth2/authorize?client_id=42&scope=bot%20applications.commands"
        );
    }
}

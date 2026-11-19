use serde::Deserialize;

use crate::discord::DiscordError;

// L'application Discord d'un créateur, lue par Discord lui-même à partir du jeton du bot :
// l'identifiant n'est donc jamais saisi à la main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotApp {
    pub id: String,
    pub name: String,
}

pub trait BotVerifier: Send + Sync + 'static {
    // `Rejected` : Discord refuse le jeton. `Http` : Discord injoignable ou réponse inattendue.
    fn verify(&self, token: &str) -> Result<BotApp, DiscordError>;

    // Le nombre approximatif de serveurs où l'application a été ajoutée, si Discord le donne.
    // `None` : Discord ne l'a pas fourni ou ne répond pas ; l'interface le dit au lieu de l'inventer.
    fn guild_count(&self, _token: &str) -> Option<u64> {
        None
    }
}

// Le champ `approximate_guild_count` de l'objet Application (jeton de bot : à confirmer en pratique).
pub fn parse_guild_count(json: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("approximate_guild_count")?
        .as_u64()
}

#[derive(Deserialize)]
struct RawApp {
    id: String,
    name: String,
}

pub fn parse_application(json: &str) -> Result<BotApp, DiscordError> {
    let raw: RawApp = serde_json::from_str(json).map_err(|_| DiscordError::Parse)?;
    if !crate::discord::is_snowflake(&raw.id) {
        return Err(DiscordError::Parse);
    }
    Ok(BotApp {
        id: raw.id,
        name: raw.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_application_gives_its_id_and_name() {
        let json = r#"{"id":"1234567895647001626","name":"Alice Bot","description":"x","bot_public":true}"#;
        assert_eq!(
            parse_application(json),
            Ok(BotApp {
                id: "1234567895647001626".to_string(),
                name: "Alice Bot".to_string()
            })
        );
    }

    #[test]
    fn the_guild_count_is_read_only_when_it_is_a_whole_number() {
        assert_eq!(
            parse_guild_count(r#"{"id":"1","approximate_guild_count":42}"#),
            Some(42)
        );
        assert_eq!(
            parse_guild_count(r#"{"approximate_guild_count":0}"#),
            Some(0)
        );
        for bad in [
            r#"{"id":"1"}"#,
            r#"{"approximate_guild_count":"42"}"#,
            r#"{"approximate_guild_count":-1}"#,
            r#"{"approximate_guild_count":null}"#,
            "nope",
        ] {
            assert_eq!(parse_guild_count(bad), None, "{bad}");
        }
    }

    #[test]
    fn ids_must_be_decimal_strings_of_a_sane_length() {
        for bad in [
            r#"{"id":"abc","name":"n"}"#,
            r#"{"id":"","name":"n"}"#,
            r#"{"id":"+1","name":"n"}"#,
            r#"{"id":5,"name":"n"}"#,
            r#"{"id":"123456789012345678901","name":"n"}"#,
            r#"{"name":"n"}"#,
            r#"{"id":"1"}"#,
            "nope",
        ] {
            assert_eq!(parse_application(bad), Err(DiscordError::Parse), "{bad}");
        }
        assert!(parse_application(r#"{"id":"12345678901234567890","name":"n"}"#).is_ok());
    }
}

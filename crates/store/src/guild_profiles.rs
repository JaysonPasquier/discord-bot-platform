use platform_core::{GuildId, ProductId, Timestamp};
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS guild_profiles (
    guild_id INTEGER NOT NULL,
    product_id INTEGER NOT NULL,
    nick TEXT,
    bio TEXT,
    avatar_set INTEGER NOT NULL,
    banner_set INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (guild_id, product_id)
) STRICT;";

// Ce que l'acheteur a demandé pour un serveur ; les images elles-mêmes ne sont pas gardées.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildProfile {
    pub nick: Option<String>,
    pub bio: Option<String>,
    pub avatar_set: bool,
    pub banner_set: bool,
    pub updated_at: Timestamp,
}

impl SqliteStore {
    pub fn save_guild_profile(
        &self,
        guild: GuildId,
        product: ProductId,
        profile: &GuildProfile,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO guild_profiles
                 (guild_id, product_id, nick, bio, avatar_set, banner_set, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (guild_id, product_id) DO UPDATE SET nick = excluded.nick,
                bio = excluded.bio, avatar_set = excluded.avatar_set,
                banner_set = excluded.banner_set, updated_at = excluded.updated_at",
            params![
                to_db(guild.0)?,
                to_db(product.0)?,
                profile.nick,
                profile.bio,
                profile.avatar_set,
                profile.banner_set,
                profile.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn guild_profile(
        &self,
        guild: GuildId,
        product: ProductId,
    ) -> Result<Option<GuildProfile>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT nick, bio, avatar_set, banner_set, updated_at FROM guild_profiles
                 WHERE guild_id = ?1 AND product_id = ?2",
                params![to_db(guild.0)?, to_db(product.0)?],
                |r| {
                    Ok(GuildProfile {
                        nick: r.get(0)?,
                        bio: r.get(1)?,
                        avatar_set: r.get(2)?,
                        banner_set: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    fn profile(nick: Option<&str>, avatar: bool, at: i64) -> GuildProfile {
        GuildProfile {
            nick: nick.map(str::to_string),
            bio: Some("about".to_string()),
            avatar_set: avatar,
            banner_set: !avatar,
            updated_at: at,
        }
    }

    #[test]
    fn a_profile_round_trips_per_server_and_bot() {
        let s = store();
        let (a, b) = (profile(Some("Rex"), true, 10), profile(None, false, 20));
        assert!(s.save_guild_profile(GuildId(1), ProductId(1), &a).is_ok());
        assert!(s.save_guild_profile(GuildId(2), ProductId(1), &b).is_ok());
        assert!(s.save_guild_profile(GuildId(1), ProductId(2), &b).is_ok());
        assert_eq!(
            s.guild_profile(GuildId(1), ProductId(1)).ok().flatten(),
            Some(a)
        );
        assert_eq!(
            s.guild_profile(GuildId(2), ProductId(1)).ok().flatten(),
            Some(b.clone())
        );
        assert_eq!(
            s.guild_profile(GuildId(1), ProductId(2)).ok().flatten(),
            Some(b)
        );
        assert!(matches!(
            s.guild_profile(GuildId(3), ProductId(1)),
            Ok(None)
        ));
    }

    #[test]
    fn saving_again_replaces_every_field() {
        let s = store();
        assert!(s
            .save_guild_profile(GuildId(1), ProductId(1), &profile(Some("old"), true, 1))
            .is_ok());
        let newer = GuildProfile {
            nick: None,
            bio: None,
            avatar_set: false,
            banner_set: true,
            updated_at: 2,
        };
        assert!(s
            .save_guild_profile(GuildId(1), ProductId(1), &newer)
            .is_ok());
        assert_eq!(
            s.guild_profile(GuildId(1), ProductId(1)).ok().flatten(),
            Some(newer)
        );
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        assert!(matches!(
            s.save_guild_profile(GuildId(u64::MAX), ProductId(1), &profile(None, true, 1)),
            Err(StoreError::IdOutOfRange)
        ));
        assert!(matches!(
            s.guild_profile(GuildId(1), ProductId(u64::MAX)),
            Err(StoreError::IdOutOfRange)
        ));
    }
}

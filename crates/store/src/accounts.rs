use platform_core::Timestamp;
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};
use crate::token::TokenHash;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users (
    discord_id INTEGER PRIMARY KEY,
    username TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS sessions (
    token_hash BLOB PRIMARY KEY,
    discord_id INTEGER NOT NULL,
    csrf TEXT NOT NULL,
    guilds TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT 0
) STRICT;";

// Colonnes ajoutées après le premier schéma : migrations idempotentes.
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), StoreError> {
    let has_created_at: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'created_at'")?
        .exists([])?;
    if !has_created_at {
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    // La table users peut ne pas encore exister si la base est très ancienne.
    let users_exists: bool = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='users'")?
        .exists([])?;
    if users_exists {
        let has_cgu: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('users') WHERE name = 'cgu_accepted_at'")?
            .exists([])?;
        if !has_cgu {
            conn.execute_batch("ALTER TABLE users ADD COLUMN cgu_accepted_at INTEGER")?;
        }
        // Champs de profil public ajoutés après le premier schéma.
        for col in ["bio", "github_url", "twitter_url", "website_url"] {
            let has: bool = conn
                .prepare(&format!(
                    "SELECT 1 FROM pragma_table_info('users') WHERE name = '{col}'"
                ))?
                .exists([])?;
            if !has {
                conn.execute_batch(&format!("ALTER TABLE users ADD COLUMN {col} TEXT"))?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub username: String,
    pub bio: Option<String>,
    pub github_url: Option<String>,
    pub twitter_url: Option<String>,
    pub website_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub discord_id: u64,
    pub username: String,
    pub csrf: String,
    pub guilds: String,
    pub cgu_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSession {
    pub id: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

impl SqliteStore {
    pub fn upsert_user(&self, discord_id: u64, username: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO users (discord_id, username) VALUES (?1, ?2)
             ON CONFLICT (discord_id) DO UPDATE SET username = excluded.username",
            params![to_db(discord_id)?, username],
        )?;
        Ok(())
    }

    // Le nom affiché d'un créateur, pour sa fiche sur le marché.
    pub fn username(&self, discord_id: u64) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT username FROM users WHERE discord_id = ?1",
                params![to_db(discord_id)?],
                |r| r.get(0),
            )
            .optional()?)
    }

    // Seul le hash du cookie de session est stocké, comme pour les jetons de container.
    // La liste des serveurs est un texte opaque pour le store ; le jeton d'accès Discord n'est jamais gardé.
    pub fn create_session(
        &self,
        hash: &TokenHash,
        discord_id: u64,
        csrf: &str,
        guilds: &str,
        expires_at: Timestamp,
        created_at: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sessions (token_hash, discord_id, csrf, guilds, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                hash.as_slice(),
                to_db(discord_id)?,
                csrf,
                guilds,
                expires_at,
                created_at,
            ],
        )?;
        Ok(())
    }

    pub fn session(&self, hash: &TokenHash, now: Timestamp) -> Result<Option<Session>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT s.discord_id, u.username, s.csrf, s.guilds,
                        CASE WHEN u.cgu_accepted_at IS NOT NULL THEN 1 ELSE 0 END
                 FROM sessions s JOIN users u ON u.discord_id = s.discord_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                params![hash.as_slice(), now],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, username, csrf, guilds, cgu)| {
            Ok(Session {
                discord_id: from_db(id)?,
                username,
                csrf,
                guilds,
                cgu_accepted: cgu != 0,
            })
        })
        .transpose()
    }

    pub fn accept_cgu(&self, discord_id: u64, now: Timestamp) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET cgu_accepted_at = ?1 WHERE discord_id = ?2 AND cgu_accepted_at IS NULL",
            params![now, to_db(discord_id)?],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, hash: &TokenHash) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM sessions WHERE token_hash = ?1",
            params![hash.as_slice()],
        )?;
        Ok(())
    }

    // Les sessions actives d'un utilisateur, les plus récentes d'abord : de quoi reconnaître
    // « c'est mon téléphone, c'est mon ordinateur » et révoquer celles qu'il ne reconnaît pas.
    // L'identifiant est le hash lui-même en hexadécimal : il ne redonne jamais le
    // cookie, seulement une clé pour le retrouver dans cette table.
    pub fn sessions_of(
        &self,
        discord_id: u64,
        now: Timestamp,
    ) -> Result<Vec<ActiveSession>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT token_hash, created_at, expires_at FROM sessions
             WHERE discord_id = ?1 AND expires_at > ?2 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![to_db(discord_id)?, now], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (hash, created_at, expires_at) = row?;
            Ok(ActiveSession {
                id: crate::token::to_hex(&hash),
                created_at,
                expires_at,
            })
        })
        .collect()
    }

    // `true` si une session de cet utilisateur portait cet identifiant (et a été supprimée) ;
    // `false` sinon, sans dire si c'est parce qu'elle n'existe pas ou parce qu'elle appartient à
    // quelqu'un d'autre (même message des deux côtés, pour ne rien révéler à un visiteur).
    pub fn revoke_session(&self, discord_id: u64, id: &str) -> Result<bool, StoreError> {
        let Some(hash) = crate::token::hex_decode(id) else {
            return Ok(false);
        };
        let changed = self.conn.execute(
            "DELETE FROM sessions WHERE token_hash = ?1 AND discord_id = ?2",
            params![hash, to_db(discord_id)?],
        )?;
        Ok(changed > 0)
    }

    pub fn purge_expired_sessions(&self, now: Timestamp) -> Result<usize, StoreError> {
        Ok(self
            .conn
            .execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now])?)
    }

    pub fn user_profile(&self, discord_id: u64) -> Result<Option<UserProfile>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT username, bio, github_url, twitter_url, website_url
                 FROM users WHERE discord_id = ?1",
                params![to_db(discord_id)?],
                |r| {
                    Ok(UserProfile {
                        username: r.get(0)?,
                        bio: r.get(1)?,
                        github_url: r.get(2)?,
                        twitter_url: r.get(3)?,
                        website_url: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_user_profile(
        &self,
        discord_id: u64,
        bio: Option<&str>,
        github_url: Option<&str>,
        twitter_url: Option<&str>,
        website_url: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE users SET bio = ?2, github_url = ?3, twitter_url = ?4, website_url = ?5
             WHERE discord_id = ?1",
            params![
                to_db(discord_id)?,
                bio,
                github_url,
                twitter_url,
                website_url
            ],
        )?;
        Ok(())
    }

    // Supprime définitivement un compte et toutes ses données (RGPD).
    pub fn delete_account(&self, discord_id: u64) -> Result<(), StoreError> {
        let id = to_db(discord_id)?;
        let transaction = self.conn.unchecked_transaction()?;
        // Supprimer toutes les souscriptions de cet utilisateur.
        transaction.execute(
            "DELETE FROM subscriptions WHERE subscriber_id = ?1",
            params![id],
        )?;
        // Révoquer toutes les sessions.
        transaction.execute("DELETE FROM sessions WHERE discord_id = ?1", params![id])?;
        // Supprimer le compte.
        transaction.execute("DELETE FROM users WHERE discord_id = ?1", params![id])?;
        Ok(transaction.commit()?)
    }

    pub fn all_users(&self) -> Result<Vec<(u64, String)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT discord_id, username FROM users ORDER BY discord_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|row| {
            let (id, name) = row?;
            Ok((from_db(id)?, name))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::hash_token;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    fn login(s: &SqliteStore, token: &str, id: u64, expires_at: Timestamp) {
        login_at(s, token, id, expires_at, 0);
    }

    fn login_at(
        s: &SqliteStore,
        token: &str,
        id: u64,
        expires_at: Timestamp,
        created_at: Timestamp,
    ) {
        assert!(s.upsert_user(id, "alice").is_ok());
        assert!(s
            .create_session(
                &hash_token(token),
                id,
                "csrf1",
                "[]",
                expires_at,
                created_at
            )
            .is_ok());
    }

    fn session(s: &SqliteStore, token: &str, now: Timestamp) -> Option<Session> {
        match s.session(&hash_token(token), now) {
            Ok(v) => v,
            Err(e) => panic!("read failed: {e}"),
        }
    }

    #[test]
    fn a_username_is_found_by_discord_id_and_unknown_ids_have_none() {
        let s = store();
        assert!(s.upsert_user(7, "carol").is_ok());
        assert!(matches!(s.username(7), Ok(Some(n)) if n == "carol"));
        assert!(matches!(s.username(8), Ok(None)));
        assert!(s.upsert_user(7, "carol2").is_ok());
        assert!(matches!(s.username(7), Ok(Some(n)) if n == "carol2"));
    }

    #[test]
    fn a_session_resolves_to_its_user() {
        let s = store();
        login(&s, "cookie", 42, 1_000);
        assert_eq!(
            session(&s, "cookie", 10),
            Some(Session {
                discord_id: 42,
                username: "alice".to_string(),
                csrf: "csrf1".to_string(),
                guilds: "[]".to_string(),
                cgu_accepted: false,
            })
        );
    }

    #[test]
    fn an_unknown_cookie_has_no_session() {
        let s = store();
        login(&s, "cookie", 42, 1_000);
        assert_eq!(session(&s, "other", 10), None);
    }

    #[test]
    fn a_session_is_valid_strictly_before_its_expiry() {
        let s = store();
        login(&s, "cookie", 42, 1_000);
        assert!(session(&s, "cookie", 999).is_some());
        assert_eq!(session(&s, "cookie", 1_000), None);
        assert_eq!(session(&s, "cookie", 2_000), None);
    }

    #[test]
    fn the_username_is_refreshed_on_each_login() {
        let s = store();
        login(&s, "cookie", 42, 1_000);
        assert!(s.upsert_user(42, "alice2").is_ok());
        assert_eq!(
            session(&s, "cookie", 10).map(|x| x.username),
            Some("alice2".to_string())
        );
    }

    #[test]
    fn a_session_needs_a_known_user() {
        let s = store();
        assert!(s
            .create_session(&hash_token("cookie"), 7, "c", "[]", 1_000, 0)
            .is_ok());
        assert_eq!(session(&s, "cookie", 10), None);
    }

    #[test]
    fn deleting_a_session_logs_out() {
        let s = store();
        login(&s, "cookie", 42, 1_000);
        login(&s, "other", 43, 1_000);
        assert!(s.delete_session(&hash_token("cookie")).is_ok());
        assert_eq!(session(&s, "cookie", 10), None);
        assert!(session(&s, "other", 10).is_some());
    }

    #[test]
    fn expired_sessions_are_purged() {
        let s = store();
        login(&s, "old", 42, 100);
        login(&s, "edge", 42, 200);
        login(&s, "new", 42, 300);
        assert!(matches!(s.purge_expired_sessions(200), Ok(2)));
        assert!(session(&s, "new", 10).is_some());
        assert_eq!(session(&s, "old", 10), None);
        assert_eq!(session(&s, "edge", 10), None);
    }

    #[test]
    fn all_users_are_listed_by_ascending_id() {
        let s = store();
        assert!(s.upsert_user(42, "bob").is_ok());
        assert!(s.upsert_user(7, "alice").is_ok());
        assert_eq!(
            s.all_users().unwrap_or_default(),
            vec![(7, "alice".to_string()), (42, "bob".to_string())]
        );
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        assert!(matches!(
            s.upsert_user(u64::MAX, "x"),
            Err(StoreError::IdOutOfRange)
        ));
    }

    #[test]
    fn active_sessions_are_listed_newest_first_and_expired_ones_are_left_out() {
        let s = store();
        login_at(&s, "first", 42, 9_000, 100);
        login_at(&s, "second", 42, 9_000, 200);
        login_at(&s, "gone", 42, 150, 50);
        login_at(&s, "other-user", 43, 9_000, 999);
        let mine = s.sessions_of(42, 500).unwrap_or_default();
        assert_eq!(mine.len(), 2);
        assert_eq!((mine[0].created_at, mine[1].created_at), (200, 100));
        assert!(mine.iter().all(|a| a.expires_at == 9_000));
    }

    #[test]
    fn revoking_a_session_logs_it_out_and_only_its_owner_can_revoke_it() {
        let s = store();
        login_at(&s, "cookie", 42, 9_000, 100);
        let id = s
            .sessions_of(42, 0)
            .ok()
            .and_then(|v| v.first().map(|a| a.id.clone()))
            .unwrap_or_default();
        // Un autre utilisateur ne peut pas révoquer la session de quelqu'un d'autre.
        assert!(matches!(s.revoke_session(43, &id), Ok(false)));
        assert!(session(&s, "cookie", 10).is_some());
        assert!(matches!(s.revoke_session(42, &id), Ok(true)));
        assert_eq!(session(&s, "cookie", 10), None);
        // Déjà révoquée : un deuxième essai ne retrouve plus rien.
        assert!(matches!(s.revoke_session(42, &id), Ok(false)));
    }

    #[test]
    fn a_malformed_session_id_is_refused_without_a_database_error() {
        let s = store();
        assert!(matches!(s.revoke_session(42, "not-hex"), Ok(false)));
        assert!(matches!(s.revoke_session(42, "ab"), Ok(false)));
    }

    #[test]
    fn accepting_cgu_flips_the_flag_and_is_idempotent() {
        let s = store();
        login(&s, "cookie", 42, 9_000);
        // Avant acceptation : flag à false.
        assert_eq!(
            session(&s, "cookie", 10).map(|x| x.cgu_accepted),
            Some(false)
        );
        assert!(s.accept_cgu(42, 1_000).is_ok());
        assert_eq!(
            session(&s, "cookie", 10).map(|x| x.cgu_accepted),
            Some(true)
        );
        // Deuxième appel : ne doit pas échouer, ne change pas l'horodatage.
        assert!(s.accept_cgu(42, 2_000).is_ok());
        assert_eq!(
            session(&s, "cookie", 10).map(|x| x.cgu_accepted),
            Some(true)
        );
    }

    // Une base créée avant l'ajout de `created_at` (fichier `.sqlite` déjà sur le disque de
    // quelqu'un) doit continuer à s'ouvrir, avec la colonne ajoutée au lieu d'une erreur.
    #[test]
    fn an_old_database_without_created_at_is_migrated_in_place() {
        let Ok(conn) = rusqlite::Connection::open_in_memory() else {
            panic!("cannot open in-memory connection");
        };
        let old_schema = "
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    discord_id INTEGER NOT NULL,
    csrf TEXT NOT NULL,
    guilds TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;";
        assert!(conn.execute_batch(old_schema).is_ok());
        assert!(migrate(&conn).is_ok());
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'created_at'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);
        assert!(has_column);
        // Ré-appliquer la migration sur une base qui a déjà la colonne ne doit pas échouer.
        assert!(migrate(&conn).is_ok());
    }
}

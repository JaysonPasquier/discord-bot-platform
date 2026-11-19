use platform_core::{ProductId, Timestamp};
use rusqlite::params;

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS github_installations (
    user_id INTEGER NOT NULL,
    installation_id INTEGER NOT NULL,
    login TEXT NOT NULL,
    selection TEXT NOT NULL,
    PRIMARY KEY (user_id, installation_id)
) STRICT;
CREATE TABLE IF NOT EXISTS product_sources (
    product_id INTEGER NOT NULL,
    tier TEXT NOT NULL,
    installation_id INTEGER NOT NULL,
    repo TEXT NOT NULL,
    branch TEXT NOT NULL,
    auto_deploy INTEGER NOT NULL,
    last_head TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (product_id, tier)
) STRICT;
CREATE TABLE IF NOT EXISTS deployments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL,
    tier TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    branch TEXT NOT NULL,
    trigger TEXT NOT NULL,
    status TEXT NOT NULL,
    image TEXT NOT NULL,
    log TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS deployments_by_bot ON deployments (product_id, tier, id);";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstallation {
    pub id: u64,
    pub login: String,
    pub selection: String,
}

// Dépôt et branche d'une version d'un bot ; une version = un dépôt (ou le même dépôt pour les deux).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub installation: u64,
    pub repo: String,
    pub branch: String,
    pub auto_deploy: bool,
    // Dernier commit de la branche déjà pris en compte : un retour en arrière ne le change pas,
    // donc l'auto-déploiement ne l'annule pas.
    pub last_head: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedSource {
    pub product: ProductId,
    pub tier: String,
    pub source: Source,
}

impl SqliteStore {
    // La liste renvoyée par GitHub fait foi : une installation retirée disparaît du compte.
    pub fn replace_installations(
        &self,
        user: u64,
        installations: &[StoredInstallation],
    ) -> Result<(), StoreError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM github_installations WHERE user_id = ?1",
            params![to_db(user)?],
        )?;
        for i in installations {
            transaction.execute(
                "INSERT INTO github_installations (user_id, installation_id, login, selection)
                 VALUES (?1, ?2, ?3, ?4)",
                params![to_db(user)?, to_db(i.id)?, i.login, i.selection],
            )?;
        }
        Ok(transaction.commit()?)
    }

    pub fn installations_of(&self, user: u64) -> Result<Vec<StoredInstallation>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT installation_id, login, selection FROM github_installations
             WHERE user_id = ?1 ORDER BY installation_id",
        )?;
        let rows = statement.query_map(params![to_db(user)?], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, login, selection) = row?;
            Ok(StoredInstallation {
                id: from_db(id)?,
                login,
                selection,
            })
        })
        .collect()
    }

    pub fn user_has_installation(&self, user: u64, installation: u64) -> Result<bool, StoreError> {
        Ok(self
            .installations_of(user)?
            .iter()
            .any(|i| i.id == installation))
    }

    pub fn save_source(
        &self,
        product: ProductId,
        tier: &str,
        source: &Source,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO product_sources (product_id, tier, installation_id, repo, branch, auto_deploy, last_head)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (product_id, tier) DO UPDATE SET installation_id = excluded.installation_id,
                repo = excluded.repo, branch = excluded.branch, auto_deploy = excluded.auto_deploy,
                last_head = excluded.last_head",
            params![
                to_db(product.0)?,
                tier,
                to_db(source.installation)?,
                source.repo,
                source.branch,
                source.auto_deploy,
                source.last_head
            ],
        )?;
        Ok(())
    }

    pub fn source(&self, product: ProductId, tier: &str) -> Result<Option<Source>, StoreError> {
        Ok(self
            .watched_where(
                "product_id = ?1 AND tier = ?2",
                params![to_db(product.0)?, tier],
            )?
            .into_iter()
            .next()
            .map(|w| w.source))
    }

    pub fn set_last_head(
        &self,
        product: ProductId,
        tier: &str,
        sha: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE product_sources SET last_head = ?3 WHERE product_id = ?1 AND tier = ?2",
            params![to_db(product.0)?, tier, sha],
        )?;
        Ok(())
    }

    pub fn delete_source(&self, product: ProductId, tier: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM product_sources WHERE product_id = ?1 AND tier = ?2",
            params![to_db(product.0)?, tier],
        )?;
        Ok(())
    }

    pub fn auto_deploy_sources(&self) -> Result<Vec<WatchedSource>, StoreError> {
        self.watched_where("auto_deploy = 1", rusqlite::params![])
    }

    // Une livraison de webhook désigne une installation, un dépôt et une branche.
    pub fn auto_deploy_sources_for(
        &self,
        installation: u64,
        repo: &str,
        branch: &str,
    ) -> Result<Vec<WatchedSource>, StoreError> {
        self.watched_where(
            "auto_deploy = 1 AND installation_id = ?1 AND repo = ?2 AND branch = ?3",
            params![to_db(installation)?, repo, branch],
        )
    }

    fn watched_where(
        &self,
        condition: &str,
        parameters: impl rusqlite::Params,
    ) -> Result<Vec<WatchedSource>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT product_id, tier, installation_id, repo, branch, auto_deploy, last_head
             FROM product_sources WHERE {condition} ORDER BY product_id, tier"
        ))?;
        let rows = statement.query_map(parameters, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (product, tier, installation, repo, branch, auto_deploy, last_head) = row?;
            Ok(WatchedSource {
                product: ProductId(from_db(product)?),
                tier,
                source: Source {
                    installation: from_db(installation)?,
                    repo,
                    branch,
                    auto_deploy,
                    last_head,
                },
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployStatus {
    Queued,
    Building,
    Running,
    Ready,
    Failed,
}

impl DeployStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Building => "building",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "queued" => Self::Queued,
            "building" => Self::Building,
            "running" => Self::Running,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    // Encore en cours : l'interface se rafraîchit seule tant qu'il en reste un.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Building)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub id: u64,
    pub product: ProductId,
    pub tier: String,
    pub commit: String,
    pub branch: String,
    pub trigger: String,
    pub status: DeployStatus,
    pub image: String,
    pub log: String,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

const MAX_LOG_BYTES: usize = 64 * 1024;

fn truncate_log(log: &str) -> &str {
    if log.len() <= MAX_LOG_BYTES {
        return log;
    }
    let mut start = log.len() - MAX_LOG_BYTES;
    while !log.is_char_boundary(start) {
        start += 1;
    }
    &log[start..]
}

type DeploymentRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
);

fn to_deployment(row: DeploymentRow) -> Result<Deployment, StoreError> {
    let (id, product, tier, commit, branch, trigger, status, image, log, created, finished) = row;
    Ok(Deployment {
        id: from_db(id)?,
        product: ProductId(from_db(product)?),
        tier,
        commit,
        branch,
        trigger,
        status: DeployStatus::parse(&status).unwrap_or(DeployStatus::Failed),
        image,
        log,
        created_at: created,
        finished_at: finished,
    })
}

const DEPLOYMENT_COLUMNS: &str =
    "id, product_id, tier, commit_sha, branch, trigger, status, image, log, created_at, finished_at";

impl SqliteStore {
    pub fn create_deployment(
        &self,
        product: ProductId,
        tier: &str,
        commit: &str,
        branch: &str,
        trigger: &str,
        now: Timestamp,
    ) -> Result<u64, StoreError> {
        self.conn.execute(
            "INSERT INTO deployments (product_id, tier, commit_sha, branch, trigger, status, image, log, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'queued', '', '', ?6)",
            params![to_db(product.0)?, tier, commit, branch, trigger, now],
        )?;
        from_db(self.conn.last_insert_rowid())
    }

    pub fn deployment(&self, id: u64) -> Result<Option<Deployment>, StoreError> {
        Ok(self
            .deployments_where("id = ?1", params![to_db(id)?], 1)?
            .into_iter()
            .next())
    }

    // Les plus récents d'abord.
    pub fn deployments_of(
        &self,
        product: ProductId,
        limit: u32,
    ) -> Result<Vec<Deployment>, StoreError> {
        self.deployments_where("product_id = ?1", params![to_db(product.0)?], limit)
    }

    pub fn latest_deployment(
        &self,
        product: ProductId,
        tier: &str,
    ) -> Result<Option<Deployment>, StoreError> {
        Ok(self
            .deployments_where(
                "product_id = ?1 AND tier = ?2",
                params![to_db(product.0)?, tier],
                1,
            )?
            .into_iter()
            .next())
    }

    pub fn running_deployment(
        &self,
        product: ProductId,
        tier: &str,
    ) -> Result<Option<Deployment>, StoreError> {
        Ok(self
            .deployments_where(
                "product_id = ?1 AND tier = ?2 AND status = 'running'",
                params![to_db(product.0)?, tier],
                1,
            )?
            .into_iter()
            .next())
    }

    pub fn deployments_with_status(
        &self,
        status: DeployStatus,
    ) -> Result<Vec<Deployment>, StoreError> {
        self.deployments_where("status = ?1", params![status.as_str()], 1000)
    }

    pub fn recent_deployments_all(&self, limit: u32) -> Result<Vec<Deployment>, StoreError> {
        self.deployments_where("1 = 1", [], limit)
    }

    pub fn set_deployment_progress(
        &self,
        id: u64,
        status: DeployStatus,
        image: &str,
        log: &str,
        finished: Option<Timestamp>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE deployments SET status = ?2, image = ?3, log = ?4, finished_at = ?5 WHERE id = ?1",
            params![to_db(id)?, status.as_str(), image, truncate_log(log), finished],
        )?;
        Ok(())
    }

    // Un nouveau déploiement en service remplace l'ancien : celui-ci reste disponible pour y revenir.
    pub fn make_current(&self, id: u64, now: Timestamp) -> Result<(), StoreError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "UPDATE deployments SET status = 'ready'
             WHERE status = 'running' AND product_id = (SELECT product_id FROM deployments WHERE id = ?1)
               AND tier = (SELECT tier FROM deployments WHERE id = ?1)",
            params![to_db(id)?],
        )?;
        transaction.execute(
            "UPDATE deployments SET status = 'running', finished_at = ?2 WHERE id = ?1",
            params![to_db(id)?, now],
        )?;
        Ok(transaction.commit()?)
    }

    fn deployments_where(
        &self,
        condition: &str,
        parameters: impl rusqlite::Params,
        limit: u32,
    ) -> Result<Vec<Deployment>, StoreError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {DEPLOYMENT_COLUMNS} FROM deployments WHERE {condition} ORDER BY id DESC LIMIT {limit}"
        ))?;
        let rows = statement.query_map(parameters, |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })?;
        rows.map(|row| to_deployment(row?)).collect()
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

    fn installation(id: u64, login: &str) -> StoredInstallation {
        StoredInstallation {
            id,
            login: login.to_string(),
            selection: "selected".to_string(),
        }
    }

    fn source(repo: &str, branch: &str, auto: bool) -> Source {
        Source {
            installation: 7,
            repo: repo.to_string(),
            branch: branch.to_string(),
            auto_deploy: auto,
            last_head: String::new(),
        }
    }

    #[test]
    fn the_installation_list_of_a_user_is_replaced_not_merged() {
        let s = store();
        assert!(s
            .replace_installations(1, &[installation(7, "a"), installation(8, "b")])
            .is_ok());
        assert!(s.replace_installations(2, &[installation(7, "a")]).is_ok());
        assert!(s.replace_installations(1, &[installation(8, "b")]).is_ok());
        assert_eq!(s.installations_of(1).ok(), Some(vec![installation(8, "b")]));
        assert_eq!(s.installations_of(2).ok(), Some(vec![installation(7, "a")]));
        assert_eq!(s.user_has_installation(1, 8).ok(), Some(true));
        assert_eq!(s.user_has_installation(1, 7).ok(), Some(false));
        assert_eq!(s.user_has_installation(3, 7).ok(), Some(false));
        assert!(s.replace_installations(1, &[]).is_ok());
        assert_eq!(s.installations_of(1).ok(), Some(vec![]));
    }

    #[test]
    fn a_source_is_kept_per_bot_and_version_and_can_be_replaced_or_removed() {
        let s = store();
        let (p, q) = (ProductId(1), ProductId(2));
        assert!(s
            .save_source(p, "free", &source("a/free", "main", false))
            .is_ok());
        assert!(s
            .save_source(p, "premium", &source("a/paid", "release", true))
            .is_ok());
        assert!(s
            .save_source(q, "free", &source("b/x", "main", false))
            .is_ok());
        assert_eq!(
            s.source(p, "free").ok().flatten(),
            Some(source("a/free", "main", false))
        );
        assert_eq!(
            s.source(p, "premium").ok().flatten(),
            Some(source("a/paid", "release", true))
        );
        assert!(s
            .save_source(p, "free", &source("a/free", "dev", true))
            .is_ok());
        assert_eq!(
            s.source(p, "free").ok().flatten(),
            Some(source("a/free", "dev", true))
        );
        assert!(s.delete_source(p, "free").is_ok());
        assert!(matches!(s.source(p, "free"), Ok(None)));
        assert!(matches!(s.source(q, "premium"), Ok(None)));
        assert!(s.source(q, "free").ok().flatten().is_some());
    }

    #[test]
    fn the_last_seen_commit_is_updated_alone() {
        let s = store();
        let p = ProductId(1);
        assert!(s
            .save_source(p, "free", &source("a/x", "main", true))
            .is_ok());
        assert!(s.set_last_head(p, "free", "abc").is_ok());
        let saved = s.source(p, "free").ok().flatten();
        assert_eq!(saved.as_ref().map(|s| s.last_head.as_str()), Some("abc"));
        assert_eq!(
            saved.map(|s| (s.repo, s.branch, s.auto_deploy)),
            Some(("a/x".to_string(), "main".to_string(), true))
        );
        assert!(s.set_last_head(ProductId(2), "free", "zzz").is_ok());
        assert!(matches!(s.source(ProductId(2), "free"), Ok(None)));
    }

    #[test]
    fn only_auto_deploy_sources_are_watched() {
        let s = store();
        assert!(s
            .save_source(ProductId(1), "free", &source("a/x", "main", false))
            .is_ok());
        assert!(s
            .save_source(ProductId(1), "premium", &source("a/x", "main", true))
            .is_ok());
        assert!(s
            .save_source(ProductId(2), "free", &source("a/y", "main", true))
            .is_ok());
        let all: Vec<_> = s
            .auto_deploy_sources()
            .unwrap_or_default()
            .into_iter()
            .map(|w| (w.product.0, w.tier))
            .collect();
        assert_eq!(all, [(1, "premium".to_string()), (2, "free".to_string())]);
        let hooked = s
            .auto_deploy_sources_for(7, "a/x", "main")
            .unwrap_or_default();
        assert_eq!(hooked.len(), 1);
        assert_eq!(hooked[0].tier, "premium");
        assert!(s
            .auto_deploy_sources_for(7, "a/x", "dev")
            .unwrap_or_default()
            .is_empty());
        assert!(s
            .auto_deploy_sources_for(8, "a/x", "main")
            .unwrap_or_default()
            .is_empty());
        assert!(s
            .auto_deploy_sources_for(7, "a/z", "main")
            .unwrap_or_default()
            .is_empty());
    }

    fn deploy(s: &SqliteStore, product: u64, tier: &str, commit: &str, at: i64) -> u64 {
        match s.create_deployment(ProductId(product), tier, commit, "main", "manual", at) {
            Ok(id) => id,
            Err(e) => panic!("create failed: {e}"),
        }
    }

    #[test]
    fn deployments_are_queued_listed_newest_first_and_found_by_id() {
        let s = store();
        let a = deploy(&s, 1, "free", "aaa", 10);
        let b = deploy(&s, 1, "premium", "bbb", 20);
        let c = deploy(&s, 2, "free", "ccc", 30);
        assert!(a < b && b < c);
        let mine: Vec<_> = s
            .deployments_of(ProductId(1), 10)
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.commit)
            .collect();
        assert_eq!(mine, ["bbb", "aaa"]);
        assert_eq!(
            s.deployments_of(ProductId(1), 1).map(|v| v.len()).ok(),
            Some(1)
        );
        let first = s.deployment(a).ok().flatten();
        assert_eq!(
            first.map(|d| (d.status, d.trigger, d.created_at, d.finished_at)),
            Some((DeployStatus::Queued, "manual".to_string(), 10, None))
        );
        assert!(matches!(s.deployment(999), Ok(None)));
        assert_eq!(
            s.latest_deployment(ProductId(1), "free")
                .ok()
                .flatten()
                .map(|d| d.id),
            Some(a)
        );
    }

    #[test]
    fn progress_updates_status_image_and_log_and_the_log_is_capped() {
        let s = store();
        let id = deploy(&s, 1, "free", "aaa", 10);
        assert!(s
            .set_deployment_progress(id, DeployStatus::Building, "img:aaa", "step 1", None)
            .is_ok());
        let d = s.deployment(id).ok().flatten();
        assert_eq!(
            d.map(|d| (d.status, d.image, d.log)),
            Some((
                DeployStatus::Building,
                "img:aaa".to_string(),
                "step 1".to_string()
            ))
        );
        let long = format!("start{}end", "é".repeat(MAX_LOG_BYTES));
        assert!(s
            .set_deployment_progress(id, DeployStatus::Failed, "", &long, Some(99))
            .is_ok());
        let d = s.deployment(id).ok().flatten();
        let log = d.as_ref().map(|d| d.log.clone()).unwrap_or_default();
        assert!(log.len() <= MAX_LOG_BYTES && log.ends_with("end") && !log.contains("start"));
        assert_eq!(d.and_then(|d| d.finished_at), Some(99));
    }

    #[test]
    fn making_a_deployment_current_demotes_only_the_previous_one_of_that_bot_version() {
        let s = store();
        let (a, b, other) = (
            deploy(&s, 1, "free", "aaa", 1),
            deploy(&s, 1, "free", "bbb", 2),
            deploy(&s, 1, "premium", "ccc", 3),
        );
        for id in [a, other] {
            assert!(s.make_current(id, 5).is_ok());
        }
        assert_eq!(
            s.running_deployment(ProductId(1), "free")
                .ok()
                .flatten()
                .map(|d| d.id),
            Some(a)
        );
        assert!(s.make_current(b, 9).is_ok());
        assert_eq!(
            s.running_deployment(ProductId(1), "free")
                .ok()
                .flatten()
                .map(|d| d.id),
            Some(b)
        );
        assert_eq!(
            s.deployment(a).ok().flatten().map(|d| d.status),
            Some(DeployStatus::Ready)
        );
        assert_eq!(
            s.running_deployment(ProductId(1), "premium")
                .ok()
                .flatten()
                .map(|d| d.id),
            Some(other)
        );
        assert_eq!(
            s.deployment(b).ok().flatten().and_then(|d| d.finished_at),
            Some(9)
        );
    }

    #[test]
    fn deployments_can_be_found_by_status() {
        let s = store();
        let a = deploy(&s, 1, "free", "aaa", 1);
        let _b = deploy(&s, 2, "free", "bbb", 2);
        assert!(s
            .set_deployment_progress(a, DeployStatus::Building, "", "", None)
            .is_ok());
        assert_eq!(
            s.deployments_with_status(DeployStatus::Queued)
                .map(|v| v.len())
                .ok(),
            Some(1)
        );
        assert_eq!(
            s.deployments_with_status(DeployStatus::Building)
                .map(|v| v.len())
                .ok(),
            Some(1)
        );
        assert_eq!(
            s.deployments_with_status(DeployStatus::Failed)
                .map(|v| v.len())
                .ok(),
            Some(0)
        );
    }

    #[test]
    fn the_log_cap_is_64_kib() {
        assert_eq!(MAX_LOG_BYTES, 65_536);
    }

    #[test]
    fn statuses_round_trip_and_unknown_text_is_refused() {
        for status in [
            DeployStatus::Queued,
            DeployStatus::Building,
            DeployStatus::Running,
            DeployStatus::Ready,
            DeployStatus::Failed,
        ] {
            assert_eq!(DeployStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(DeployStatus::parse("nope"), None);
        assert!(DeployStatus::Queued.is_active() && DeployStatus::Building.is_active());
        assert!(!DeployStatus::Running.is_active() && !DeployStatus::Ready.is_active());
        assert!(!DeployStatus::Failed.is_active());
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        assert!(matches!(
            s.save_source(ProductId(u64::MAX), "free", &source("a/b", "main", false)),
            Err(StoreError::IdOutOfRange)
        ));
        assert!(matches!(
            s.replace_installations(u64::MAX, &[installation(1, "a")]),
            Err(StoreError::IdOutOfRange)
        ));
    }
}

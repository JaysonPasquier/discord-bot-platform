use platform_core::{ProductId, Timestamp};
use rusqlite::params;

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL,
    reporter_discord_id INTEGER,
    reporter_name TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL CHECK(kind IN ('bug', 'feedback')),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    dev_response TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: i64,
    pub product_id: ProductId,
    pub reporter_discord_id: Option<u64>,
    pub reporter_name: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub dev_response: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

// Regroupe les champs soumis par l'auteur du report, séparés de `now` (l'horloge de l'appelant).
pub struct NewReport<'a> {
    pub product_id: ProductId,
    pub reporter_discord_id: Option<u64>,
    pub reporter_name: &'a str,
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

impl SqliteStore {
    pub fn create_report(&self, report: NewReport<'_>, now: Timestamp) -> Result<i64, StoreError> {
        let reporter_id = report.reporter_discord_id.map(to_db).transpose()?;
        self.conn.execute(
            "INSERT INTO reports (product_id, reporter_discord_id, reporter_name, kind, title, body, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?7)",
            params![to_db(report.product_id.0)?, reporter_id, report.reporter_name, report.kind, report.title, report.body, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    // Nombre de reports soumis par un utilisateur pour un produit donné depuis `since`.
    pub fn count_recent_reports_by(
        &self,
        product_id: ProductId,
        reporter_discord_id: u64,
        since: i64,
    ) -> Result<u32, StoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reports WHERE product_id = ?1 AND reporter_discord_id = ?2 AND created_at >= ?3",
            params![to_db(product_id.0)?, to_db(reporter_discord_id)?, since],
            |r| r.get(0),
        )?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    pub fn reports_for_product(&self, product_id: ProductId) -> Result<Vec<Report>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, product_id, reporter_discord_id, reporter_name, kind, title, body, status, dev_response, created_at, updated_at
             FROM reports WHERE product_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![to_db(product_id.0)?], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
            ))
        })?;
        rows.map(|r| {
            let (
                id,
                product_id,
                rid,
                rname,
                kind,
                title,
                body,
                status,
                dev_response,
                created_at,
                updated_at,
            ) = r?;
            Ok(Report {
                id,
                product_id: ProductId(from_db(product_id)?),
                reporter_discord_id: rid.map(from_db).transpose()?,
                reporter_name: rname,
                kind,
                title,
                body,
                status,
                dev_response,
                created_at,
                updated_at,
            })
        })
        .collect()
    }

    pub fn update_report(
        &self,
        report_id: i64,
        product_id: ProductId,
        status: &str,
        dev_response: &str,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let updated = self.conn.execute(
            "UPDATE reports SET status = ?1, dev_response = ?2, updated_at = ?3
             WHERE id = ?4 AND product_id = ?5",
            params![status, dev_response, now, report_id, to_db(product_id.0)?],
        )?;
        Ok(updated > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ok;

    fn store() -> SqliteStore {
        ok(SqliteStore::in_memory(), "in-memory store")
    }

    fn new_report<'a>(
        product_id: ProductId,
        reporter_discord_id: Option<u64>,
        reporter_name: &'a str,
        kind: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> NewReport<'a> {
        NewReport {
            product_id,
            reporter_discord_id,
            reporter_name,
            kind,
            title,
            body,
        }
    }

    #[test]
    fn create_and_list_reports() {
        let s = store();
        let p = ProductId(1);
        ok(
            s.create_report(
                new_report(
                    p,
                    Some(42),
                    "Alice",
                    "bug",
                    "Crash on startup",
                    "Bot crashes",
                ),
                1000,
            ),
            "create",
        );
        ok(
            s.create_report(
                new_report(p, None, "", "feedback", "Nice bot!", "Love it"),
                1001,
            ),
            "create",
        );
        let list = ok(s.reports_for_product(p), "list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].title, "Nice bot!");
        assert_eq!(list[1].title, "Crash on startup");
        assert_eq!(list[1].reporter_discord_id, Some(42));
        assert_eq!(list[1].status, "open");
    }

    #[test]
    fn update_report_changes_status_and_response() {
        let s = store();
        let p = ProductId(1);
        let id = ok(
            s.create_report(new_report(p, None, "", "bug", "T", "B"), 1000),
            "create",
        );
        assert!(ok(
            s.update_report(id, p, "fixed", "Done in v1.2", 2000),
            "update"
        ));
        let list = ok(s.reports_for_product(p), "list");
        assert_eq!(list[0].status, "fixed");
        assert_eq!(list[0].dev_response, "Done in v1.2");
        assert_eq!(list[0].updated_at, 2000);
    }

    #[test]
    fn wrong_product_cannot_update_another_products_report() {
        let s = store();
        let p1 = ProductId(1);
        let p2 = ProductId(2);
        let id = ok(
            s.create_report(new_report(p1, None, "", "bug", "T", "B"), 1000),
            "create",
        );
        assert!(!ok(s.update_report(id, p2, "fixed", "", 2000), "update"));
        let list = ok(s.reports_for_product(p1), "list");
        assert_eq!(list[0].status, "open");
    }

    #[test]
    fn reports_are_isolated_by_product() {
        let s = store();
        ok(
            s.create_report(new_report(ProductId(1), None, "", "bug", "T", "B"), 1000),
            "create",
        );
        assert_eq!(ok(s.reports_for_product(ProductId(2)), "list").len(), 0);
    }
}

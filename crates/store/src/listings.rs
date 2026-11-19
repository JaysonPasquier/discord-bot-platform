// La page publique d'un bot, telle que son créateur la règle : texte, catégorie, images, commandes, formules.
use platform_core::{ProductId, Timestamp};
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS listings (
    product_id INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    category TEXT NOT NULL,
    avatar BLOB,
    avatar_mime TEXT,
    banner BLOB,
    banner_mime TEXT,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS listing_commands (
    product_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    PRIMARY KEY (product_id, position)
) STRICT;
CREATE TABLE IF NOT EXISTS listing_plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    price_cents INTEGER NOT NULL,
    days INTEGER NOT NULL
) STRICT;";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Avatar,
    Banner,
}

impl ImageKind {
    // Les colonnes sont choisies ici : jamais un nom venu d'une requête dans le SQL.
    fn columns(self) -> (&'static str, &'static str) {
        match self {
            Self::Avatar => ("avatar", "avatar_mime"),
            Self::Banner => ("banner", "banner_mime"),
        }
    }
}

// Sans les images elles-mêmes : on sait seulement si elles existent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub description: String,
    pub category: String,
    pub has_avatar: bool,
    pub has_banner: bool,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub id: u64,
    pub name: String,
    pub price_cents: u32,
    pub days: u32,
}

impl SqliteStore {
    pub fn listing(&self, product: ProductId) -> Result<Option<Listing>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT description, category, avatar IS NOT NULL, banner IS NOT NULL, updated_at
                 FROM listings WHERE product_id = ?1",
                params![to_db(product.0)?],
                |r| {
                    Ok(Listing {
                        description: r.get(0)?,
                        category: r.get(1)?,
                        has_avatar: r.get(2)?,
                        has_banner: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // Les images ne bougent pas ici : seul le texte est remplacé.
    pub fn save_listing_text(
        &self,
        product: ProductId,
        description: &str,
        category: &str,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO listings (product_id, description, category, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (product_id) DO UPDATE SET description = excluded.description,
                category = excluded.category, updated_at = excluded.updated_at",
            params![to_db(product.0)?, description, category, now],
        )?;
        Ok(())
    }

    pub fn set_listing_image(
        &self,
        product: ProductId,
        kind: ImageKind,
        mime: &str,
        bytes: &[u8],
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let (data, mime_column) = kind.columns();
        self.conn.execute(
            &format!(
                "INSERT INTO listings (product_id, description, category, {data}, {mime_column}, updated_at)
                 VALUES (?1, '', 'other', ?2, ?3, ?4)
                 ON CONFLICT (product_id) DO UPDATE SET {data} = excluded.{data},
                    {mime_column} = excluded.{mime_column}, updated_at = excluded.updated_at"
            ),
            params![to_db(product.0)?, bytes, mime, now],
        )?;
        Ok(())
    }

    pub fn clear_listing_image(
        &self,
        product: ProductId,
        kind: ImageKind,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let (data, mime_column) = kind.columns();
        self.conn.execute(
            &format!(
                "UPDATE listings SET {data} = NULL, {mime_column} = NULL, updated_at = ?2
                 WHERE product_id = ?1"
            ),
            params![to_db(product.0)?, now],
        )?;
        Ok(())
    }

    pub fn listing_image(
        &self,
        product: ProductId,
        kind: ImageKind,
    ) -> Result<Option<(String, Vec<u8>)>, StoreError> {
        let (data, mime_column) = kind.columns();
        let row = self
            .conn
            .query_row(
                &format!("SELECT {mime_column}, {data} FROM listings WHERE product_id = ?1"),
                params![to_db(product.0)?],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.and_then(|(mime, bytes)| Some((mime?, bytes?))))
    }

    pub fn replace_commands(
        &self,
        product: ProductId,
        commands: &[ListedCommand],
    ) -> Result<(), StoreError> {
        let id = to_db(product.0)?;
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM listing_commands WHERE product_id = ?1",
            params![id],
        )?;
        for (position, c) in commands.iter().enumerate() {
            transaction.execute(
                "INSERT INTO listing_commands (product_id, position, name, description)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, to_db(position as u64)?, c.name, c.description],
            )?;
        }
        Ok(transaction.commit()?)
    }

    pub fn commands_of(&self, product: ProductId) -> Result<Vec<ListedCommand>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT name, description FROM listing_commands WHERE product_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![to_db(product.0)?], |r| {
            Ok(ListedCommand {
                name: r.get(0)?,
                description: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // Les formules gardent leur identifiant tant qu'elles existent ; le remplacement complet en crée de nouveaux.
    pub fn replace_plans(&self, product: ProductId, plans: &[Plan]) -> Result<(), StoreError> {
        let id = to_db(product.0)?;
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM listing_plans WHERE product_id = ?1",
            params![id],
        )?;
        for (position, p) in plans.iter().enumerate() {
            transaction.execute(
                "INSERT INTO listing_plans (product_id, position, name, price_cents, days)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, to_db(position as u64)?, p.name, p.price_cents, p.days],
            )?;
        }
        Ok(transaction.commit()?)
    }

    pub fn plans_of(&self, product: ProductId) -> Result<Vec<Plan>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, price_cents, days FROM listing_plans
             WHERE product_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![to_db(product.0)?], |r| {
            Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.map(|row| {
            let (id, name, price_cents, days) = row?;
            Ok(Plan {
                id: from_db(id)?,
                name,
                price_cents,
                days,
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: ProductId = ProductId(1);

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    #[test]
    fn a_product_has_no_listing_until_the_creator_writes_one() {
        assert!(matches!(store().listing(P), Ok(None)));
    }

    #[test]
    fn text_is_replaced_and_images_are_left_alone() {
        let s = store();
        assert!(s
            .set_listing_image(P, ImageKind::Avatar, "image/png", &[1, 2, 3], 5)
            .is_ok());
        assert!(s.save_listing_text(P, "hello", "music", 6).is_ok());
        assert!(s.save_listing_text(P, "again", "games", 7).is_ok());
        let l = s
            .listing(P)
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("no listing"));
        assert_eq!(
            (l.description.as_str(), l.category.as_str(), l.updated_at),
            ("again", "games", 7)
        );
        assert!(l.has_avatar && !l.has_banner);
    }

    #[test]
    fn an_image_round_trips_and_can_be_removed() {
        let s = store();
        assert!(s
            .set_listing_image(P, ImageKind::Banner, "image/gif", &[9, 9], 1)
            .is_ok());
        assert_eq!(
            s.listing_image(P, ImageKind::Banner).ok().flatten(),
            Some(("image/gif".into(), vec![9, 9]))
        );
        assert!(matches!(s.listing_image(P, ImageKind::Avatar), Ok(None)));
        assert!(s.clear_listing_image(P, ImageKind::Banner, 2).is_ok());
        assert!(matches!(s.listing_image(P, ImageKind::Banner), Ok(None)));
        assert!(matches!(s.listing(P), Ok(Some(l)) if !l.has_banner && l.updated_at == 2));
    }

    #[test]
    fn commands_and_plans_keep_their_order_and_are_replaced_whole() {
        let s = store();
        let cmd = |n: &str| ListedCommand {
            name: n.into(),
            description: format!("does {n}"),
        };
        assert!(s.replace_commands(P, &[cmd("b"), cmd("a")]).is_ok());
        assert!(s.replace_commands(ProductId(2), &[cmd("other")]).is_ok());
        let names: Vec<_> = s
            .commands_of(P)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["b", "a"]);
        assert!(s.replace_commands(P, &[cmd("c")]).is_ok());
        assert_eq!(s.commands_of(P).unwrap_or_default().len(), 1);
        assert_eq!(s.commands_of(ProductId(2)).unwrap_or_default().len(), 1);
        let plan = |n: &str, c: u32| Plan {
            id: 0,
            name: n.into(),
            price_cents: c,
            days: 30,
        };
        assert!(s
            .replace_plans(P, &[plan("Pro", 500), plan("Max", 900)])
            .is_ok());
        let plans = s.plans_of(P).unwrap_or_default();
        assert_eq!(
            plans.iter().map(|p| p.price_cents).collect::<Vec<_>>(),
            [500, 900]
        );
        assert_ne!(plans[0].id, plans[1].id);
    }
}

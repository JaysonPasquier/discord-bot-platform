// La documentation : du contenu écrit par la plateforme (jamais saisi par un utilisateur), servi en
// HTML déjà prêt. Le frontend l'affiche tel quel dans son habillage.
use axum::Json;
use serde::Serialize;

use crate::docs::{PAGES, SDK_BODY};

#[derive(Serialize)]
pub(crate) struct DocPageDto {
    slug: &'static str,
    group: &'static str,
    title: &'static str,
    lead: &'static str,
    body: &'static str,
}

#[derive(Serialize)]
pub(crate) struct Docs {
    pages: Vec<DocPageDto>,
    sdk: &'static str,
}

pub(crate) async fn read() -> Json<Docs> {
    Json(Docs {
        pages: PAGES
            .iter()
            .map(|p| DocPageDto {
                slug: p.slug,
                group: p.group,
                title: p.title,
                lead: p.lead,
                body: p.body,
            })
            .collect(),
        sdk: SDK_BODY,
    })
}
